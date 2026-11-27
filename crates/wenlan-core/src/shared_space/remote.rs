// SPDX-License-Identifier: Apache-2.0
//! A shared Space synced through a team server instead of a sync tool.
//!
//! The server keeps the Space's folder (`crate::team`). This Wenlan keeps a
//! copy of it under the data root and runs the ordinary pass on that copy:
//!
//! ```text
//! pull   server folder ──▶ copy     every file the copy does not have yet,
//!                                    and space.json (the server's member list)
//! pass   copy ⇄ this Wenlan's pages  (unchanged)
//! push   copy ──▶ server            this member's version files the server
//!                                    does not have yet
//! ```
//!
//! Version files never change and are named by their sha256, so a file is
//! fetched or sent once. The server takes only this member's own versions.
//! A Read member can send a proposal to the hosted Rules master. Other
//! uploads require a Write grant.
//!
//! Plain `http://` is used only for an address that does not leave a
//! private network (this computer, a LAN, Tailscale); anything else must be
//! `https://`: the member key goes with every call.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use wenlan_types::team::{TeamAccess, TeamFileList, TeamMe};

use super::{version, HostLink, OUTDATED_HOST_EDIT_MESSAGE, SPACE_FILE};
use crate::error::WenlanError;
use crate::rulesync_team::{self, TeamRuleLayer};

/// This Wenlan's copies of team servers' folders, beside the settings file.
pub const MIRROR_DIR: &str = "team-mirror";

const TIMEOUT: Duration = Duration::from_secs(30);
const LIBRARY_TIMEOUT: Duration = Duration::from_secs(5);
const PARALLEL: usize = 8;
/// The largest version file sent or taken.
pub const MAX_VERSION_BYTES: usize = 8 * 1024 * 1024;

/// Where this Wenlan keeps its copy of `link`'s folder.
pub fn mirror_folder(settings_path: &Path, url: &str, space: &str) -> PathBuf {
    let id = version::sha256_hex(format!("{url}\n{space}").as_bytes());
    settings_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(MIRROR_DIR)
        .join(&id[..24])
}

/// Checks a team server's address: `https://`, or `http://` to an address
/// inside a private network. Returns it without a trailing slash.
pub fn check_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("{url} is not a server address"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("the server address must not hold a name or password".into());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("the server address must not hold a query".into());
    }
    match parsed.scheme() {
        "https" => {}
        "http" if parsed.host_str().is_some_and(is_private_host) => {}
        "http" => {
            return Err(format!(
                "{url} is not encrypted: use an https:// address, or reach the server over a private network such as Tailscale"
            ))
        }
        _ => return Err(format!("{url} is not an http(s) address")),
    }
    Ok(url.to_string())
}

fn is_private_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                // 100.64.0.0/10: Tailscale and other carrier-grade NAT.
                || (a == 100 && (64..128).contains(&b))
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback()
                // fc00::/7 unique local (Tailscale uses fd7a:115c:a1e0::/48),
                // fe80::/10 link local.
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
        Err(_) => {
            let name = bare.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost"
                || name.ends_with(".localhost")
                || name.ends_with(".local")
                || name.ends_with(".ts.net")
                || name.ends_with(".internal")
                || name.ends_with(".lan")
                || name.ends_with(".home.arpa")
                // A one-label name only resolves inside a network.
                || !name.contains('.')
        }
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("cannot reach the team server: {error}"))
}

fn library_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(LIBRARY_TIMEOUT)
        .timeout(LIBRARY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("cannot reach the team server: {error}"))
}

fn space_url(url: &str, space: &str, rest: &str) -> String {
    let mut out = format!("{url}/team/v1/spaces/");
    out.push_str(&encode_path_segment(space));
    out.push_str(rest);
    out
}

/// Percent-encodes one path segment.
pub fn urlencoding_component(text: &str) -> Result<String, &'static str> {
    if text.is_empty() {
        return Err("path id must not be empty");
    }
    if text == "." || text == ".." {
        return Err("path id must not be a dot segment");
    }
    Ok(encode_path_segment(text))
}

fn encode_path_segment(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Why a call to the server did not work, in words for the pause reason.
fn refused(url: &str, status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "the team server no longer accepts this member key (removed from the team, or a new key was made)".into(),
        403 => "the team server does not let this member use this Space".into(),
        404 => "the team server does not host this Space".into(),
        code => format!("the team server at {url} answered {code}"),
    }
}

fn unreachable(url: &str, error: reqwest::Error) -> String {
    if error.is_timeout() {
        format!("the team server at {url} did not answer in time")
    } else {
        format!("the team server at {url} cannot be reached")
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SyncError {
    #[error("{0}")]
    Revoked(String),
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{OUTDATED_HOST_EDIT_MESSAGE}")]
    OutdatedHost(Vec<String>),
}

impl From<String> for SyncError {
    fn from(reason: String) -> Self {
        Self::Unavailable(reason)
    }
}

impl SyncError {
    fn priority(&self) -> u8 {
        match self {
            Self::Revoked(_) => 2,
            Self::Refused(_) => 1,
            Self::Unavailable(_) => 0,
            Self::OutdatedHost(_) => 1,
        }
    }
}

fn response_error(url: &str, status: reqwest::StatusCode) -> SyncError {
    let reason = refused(url, status);
    match status {
        reqwest::StatusCode::UNAUTHORIZED => SyncError::Revoked(reason),
        reqwest::StatusCode::FORBIDDEN => SyncError::Refused(reason),
        _ => SyncError::Unavailable(reason),
    }
}

fn proposal_response_error(is_proposal: bool, status: reqwest::StatusCode, _body: &str) -> bool {
    is_proposal && status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
}

/// Who `key` belongs to on the server at `url`.
pub async fn me(url: &str, key: &str) -> Result<TeamMe, String> {
    me_sync(url, key).await.map_err(|error| error.to_string())
}

pub(super) async fn me_sync(url: &str, key: &str) -> Result<TeamMe, SyncError> {
    let url = check_url(url)?;
    let response = client()?
        .get(format!("{url}/team/v1/me"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|error| unreachable(&url, error))?;
    if !response.status().is_success() {
        return Err(response_error(&url, response.status()));
    }
    response
        .json::<TeamMe>()
        .await
        .map_err(|_| format!("{url} is not a Wenlan team server").into())
}

/// A call to the team's management routes (`/team/v1/admin…`), made for
/// its owner from their own computer. `path` is what follows `/admin`
/// (`""`, `/members`, `/members/<id>/key?url=…`). The server's error text
/// is passed on.
pub async fn admin(
    url: &str,
    key: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    call_team(url, key, method, &format!("/team/v1/admin{path}"), body).await
}

/// One call to the member's own account on a team server
/// (`/team/v1/account<path>`), with their member key: their AI tools.
pub async fn account(
    url: &str,
    key: &str,
    method: reqwest::Method,
    path: &str,
) -> Result<serde_json::Value, String> {
    call_team(url, key, method, &format!("/team/v1/account{path}"), None).await
}

/// Reads one member-visible team-library endpoint with the member key. The
/// caller supplies only a path below `/team/v1/library`; this helper validates
/// the team's URL and never follows redirects, so the key is sent only to the
/// joined team's own address.
pub async fn library<T: serde::de::DeserializeOwned>(
    url: &str,
    key: &str,
    path: &str,
) -> Result<T, String> {
    let url = check_url(url)?;
    let endpoint = format!("{url}/team/v1/library{path}");
    let response = library_client()?
        .get(endpoint)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|error| unreachable(&url, error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => refused(&url, status),
            403 => "the team server does not let this member use the team library".into(),
            _ => refused(&url, status),
        });
    }
    response
        .json::<T>()
        .await
        .map_err(|_| format!("{url} is not a Wenlan team server"))
}

async fn call_team(
    url: &str,
    key: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let url = check_url(url)?;
    let mut request = client()?
        .request(method, format!("{url}{path}"))
        .bearer_auth(key);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| unreachable(&url, error))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(match status.as_u16() {
            401 => refused(&url, status),
            403 => "only the team's owner can change the team".into(),
            _ if !text.trim().is_empty() => text.trim().chars().take(400).collect(),
            _ => refused(&url, status),
        });
    }
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| format!("{url} is not a Wenlan team server"))
}

/// Copies the ordinary Space files and reads the manifest-approved Rules
/// version separately. Team Rules do not enter the general page mirror.
pub(super) async fn pull(
    link: &HostLink,
    folder: &Path,
) -> Result<(TeamFileList, Option<TeamRuleLayer>), SyncError> {
    let url = check_url(&link.url)?;
    let client = client()?;
    let response = client
        .get(space_url(&url, &link.space, "/files"))
        .bearer_auth(&link.key)
        .send()
        .await
        .map_err(|error| unreachable(&url, error))?;
    if !response.status().is_success() {
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SyncError::Refused(refused(&url, response.status())));
        }
        return Err(response_error(&url, response.status()));
    }
    let list: TeamFileList = response
        .json()
        .await
        .map_err(|_| format!("{url} is not a Wenlan team server"))?;
    if !list.files.iter().any(|path| path == SPACE_FILE) {
        return Err(SyncError::Unavailable(
            "the team server has not shared this Space yet".into(),
        ));
    }
    let rules = if let Some(manifest) = &list.rules {
        let bytes = fetch_file(&client, &url, link, &manifest.file).await?;
        rulesync_team::pulled_layer(link, &list, &bytes).map_err(|error| error.to_string())?
    } else {
        None
    };
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;

    let wanted: Vec<(String, PathBuf)> = list
        .files
        .iter()
        .filter(|path| {
            !path
                .split('/')
                .nth(3)
                .is_some_and(rulesync_team::is_master_page)
        })
        .filter_map(|path| Some((path.clone(), crate::team::served_path(folder, path)?)))
        .filter(|(path, local)| path == SPACE_FILE || !version::version_file_exists(local))
        .collect();
    let fetched: Vec<Result<(), SyncError>> = futures::stream::iter(wanted)
        .map(|(path, local)| {
            let client = client.clone();
            let url = url.clone();
            async move {
                let bytes = fetch_file(&client, &url, link, &path).await?;
                tokio::task::spawn_blocking(move || store(&path, &local, &bytes))
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(Into::into)
            }
        })
        .buffer_unordered(PARALLEL)
        .collect()
        .await;
    if let Some(error) = fetched
        .into_iter()
        .filter_map(Result::err)
        .max_by_key(SyncError::priority)
    {
        return Err(error);
    }
    Ok((list, rules))
}

async fn fetch_file(
    client: &reqwest::Client,
    url: &str,
    link: &HostLink,
    path: &str,
) -> Result<Vec<u8>, SyncError> {
    let response = client
        .get(space_url(url, &link.space, "/file"))
        .query(&[("path", path)])
        .bearer_auth(&link.key)
        .send()
        .await
        .map_err(|error| unreachable(url, error))?;
    if !response.status().is_success() {
        return Err(response_error(url, response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| unreachable(url, error))?;
    if bytes.len() > MAX_VERSION_BYTES {
        return Err(format!("{path} is too large").into());
    }
    Ok(bytes.to_vec())
}

/// Writes one fetched file into the copy. A version file whose bytes do not
/// match its name is not kept (the pass would skip it anyway).
fn store(path: &str, local: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let io = |error: std::io::Error| format!("{}: {error}", local.display());
    if path != SPACE_FILE {
        let name = local
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if version::version_sha(name) != Some(version::sha256_hex(bytes).as_str()) {
            log::warn!(
                "[shared_space] {path} from the team server does not match its name; not kept"
            );
            return Ok(());
        }
    } else if std::fs::read(local).is_ok_and(|have| have == bytes) {
        return Ok(());
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
            Err(_) if local.exists() => {}
            Err(error) => return Err(io(error.error)),
        }
    }
    Ok(())
}

/// Sends this member's version files the server does not list yet.
/// Returns how many were sent. Members with Read access send Rules proposals.
pub(super) async fn push(
    link: &HostLink,
    folder: &Path,
    member: &str,
    listed: &TeamFileList,
    rules_edit_proposals: &[String],
) -> Result<u32, SyncError> {
    if listed.access == TeamAccess::Read && listed.rules.is_none() {
        return Ok(0);
    }
    let url = check_url(&link.url)?;
    // By path without extension: the server may hold a version under the
    // old `.md` name that this computer has as `.wlv`, or the other way.
    let have: HashSet<&str> = listed
        .files
        .iter()
        .map(|path| version::version_key(path))
        .collect();
    let refused: HashSet<&str> = listed
        .refused_changes
        .iter()
        .map(|receipt| receipt.change.as_str())
        .collect();
    let mine = {
        let folder = folder.to_path_buf();
        let member = member.to_string();
        tokio::task::spawn_blocking(move || own_versions(&folder, &member))
            .await
            .map_err(|error| error.to_string())?
    };
    let client = client()?;
    let missing: Vec<(String, String, PathBuf)> = mine
        .into_iter()
        .filter(|(path, page, file)| {
            let already_refused = file
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(version::version_sha)
                .is_some_and(|hash| refused.contains(hash));
            if already_refused {
                return false;
            }
            if rulesync_team::is_master_page(page) {
                let is_proposal = path.split('/').nth(2) == Some(version::PROPOSALS_DIR);
                if !is_proposal
                    || listed
                        .rules
                        .as_ref()
                        .is_none_or(|rules| rules.page_id != *page)
                {
                    return false;
                }
            } else if listed.access == TeamAccess::Read {
                return false;
            }
            !have.contains(version::version_key(path))
        })
        .collect();
    let rules_page = listed.rules.as_ref().map(|rules| rules.page_id.clone());
    let rules_edit_proposals: HashSet<&str> =
        rules_edit_proposals.iter().map(String::as_str).collect();
    let sent: Vec<Result<(), SyncError>> = futures::stream::iter(missing)
        .map(|(path, page, local)| {
            let client = client.clone();
            let url = url.clone();
            let rules_page = rules_page.clone();
            let rules_edit_proposals = &rules_edit_proposals;
            async move {
                let bytes = std::fs::read(&local).map_err(|error| error.to_string())?;
                if bytes.len() > MAX_VERSION_BYTES {
                    log::warn!("[shared_space] {path} is too large for the team server; not sent");
                    return Ok(());
                }
                let name = local
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
                let response = client
                    .put(space_url(
                        &url,
                        &link.space,
                        &format!("/versions/{page}/{name}"),
                    ))
                    .bearer_auth(&link.key)
                    .body(bytes)
                    .send()
                    .await
                    .map_err(|error| unreachable(&url, error))?;
                if response.status().is_success() {
                    Ok(())
                } else if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    if proposal_response_error(
                        path.split('/').nth(2) == Some(version::PROPOSALS_DIR)
                            && rulesync_team::is_master_page(&page)
                            && rules_page.as_deref() == Some(page.as_str())
                            && version::version_sha(&name)
                                .is_some_and(|id| rules_edit_proposals.contains(id)),
                        status,
                        &body,
                    ) {
                        let id = version::version_sha(&name)
                            .expect("queued proposal has a version id")
                            .to_owned();
                        log::warn!("[shared_space] proposal {id} refused with status {status}");
                        return Err(SyncError::OutdatedHost(vec![id]));
                    }
                    // A version the server cannot take as it is: sending it
                    // again would not change that.
                    log::warn!("[shared_space] the team server did not take {path}");
                    Ok(())
                } else {
                    Err(response_error(&url, response.status()))
                }
            }
        })
        .buffer_unordered(PARALLEL)
        .collect()
        .await;
    let count = sent.len() as u32;
    if let Some(error) = upload_failure(sent) {
        return Err(error);
    }
    Ok(count)
}

fn upload_failure(sent: Vec<Result<(), SyncError>>) -> Option<SyncError> {
    let mut refused = Vec::new();
    let mut other = Vec::new();
    for error in sent.into_iter().filter_map(Result::err) {
        match error {
            SyncError::OutdatedHost(ids) => refused.extend(ids),
            error => other.push(error),
        }
    }
    if let Some(index) = other
        .iter()
        .position(|error| matches!(error, SyncError::Revoked(_)))
    {
        return Some(other.swap_remove(index));
    }
    if !refused.is_empty() {
        refused.sort();
        refused.dedup();
        return Some(SyncError::OutdatedHost(refused));
    }
    other.into_iter().max_by_key(SyncError::priority)
}

/// This member's version files in the copy: (path as the server lists it,
/// page id, file).
fn own_versions(folder: &Path, member: &str) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    for kind in version::KIND_DIRS {
        own_files(folder, member, kind, &mut out);
    }
    out.sort();
    out
}

/// [`own_versions`] of one folder kind.
fn own_files(folder: &Path, member: &str, kind: &str, out: &mut Vec<(String, String, PathBuf)>) {
    let pages = folder.join(version::MEMBERS_DIR).join(member).join(kind);
    let Ok(entries) = std::fs::read_dir(&pages) else {
        return;
    };
    for page in entries.flatten() {
        let Ok(page_id) = page.file_name().into_string() else {
            continue;
        };
        if !version::is_safe_id(&page_id) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(page.path()) else {
            continue;
        };
        for file in files.flatten() {
            let Ok(name) = file.file_name().into_string() else {
                continue;
            };
            if version::version_sha(&name).is_some() {
                out.push((
                    format!("{}/{member}/{kind}/{page_id}/{name}", version::MEMBERS_DIR),
                    page_id.clone(),
                    file.path(),
                ));
            }
        }
    }
}

/// Turns a refusal to read or write into an error for a route.
pub fn as_error(reason: String) -> WenlanError {
    WenlanError::Validation(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_team_host_refusal_has_one_update_line() {
        let fake_host_body = "The Rules operation could not be completed.";
        assert!(proposal_response_error(
            true,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            fake_host_body
        ));
        assert!(!proposal_response_error(
            true,
            reqwest::StatusCode::BAD_REQUEST,
            fake_host_body
        ));
        assert!(!proposal_response_error(
            false,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            fake_host_body
        ));
        assert_eq!(
            SyncError::OutdatedHost(vec!["a".repeat(64)]).to_string(),
            OUTDATED_HOST_EDIT_MESSAGE
        );
    }

    #[tokio::test]
    async fn ordinary_page_proposal_422_does_not_claim_a_rules_host_update() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let scratch = tempfile::tempdir().unwrap();
        let folder = scratch.path();
        let member = "test-member";
        let page = "ordinary-page";
        let (id, _) = version::write_file(
            folder,
            member,
            version::PROPOSALS_DIR,
            page,
            "ordinary proposal",
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let host = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let bytes = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..bytes]);
            assert!(request.contains(&format!("/versions/{page}/{id}.wlv")));
            let body = "The Rules operation could not be completed.";
            let response = format!(
                "HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let listed = TeamFileList {
            access: TeamAccess::Write,
            files: Vec::new(),
            deleted_pages: Vec::new(),
            refused_changes: Vec::new(),
            rules: None,
        };
        let result = push(
            &HostLink {
                key: "fake-key".into(),
                url,
                space: "Team".into(),
                space_id: None,
            },
            folder,
            member,
            &listed,
            &[],
        )
        .await;
        host.await.unwrap();
        assert_eq!(result.unwrap(), 1);
    }

    #[tokio::test]
    async fn page_path_rules_proposal_422_does_not_pause_without_a_sent_edit() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use wenlan_types::team::{TeamInfo, TeamRulesManifest};
        let scratch = tempfile::tempdir().unwrap();
        let page = format!("team-rules-{}", "a".repeat(64));
        let (id, _) = version::write_file(
            scratch.path(),
            "member",
            version::PROPOSALS_DIR,
            &page,
            "page path proposal",
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requested_page = page.clone();
        let host = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let bytes = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..bytes])
                .contains(&format!("/versions/{requested_page}/{id}.wlv")));
            stream.write_all(b"HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let listed = TeamFileList {
            access: TeamAccess::Read,
            files: Vec::new(),
            deleted_pages: Vec::new(),
            refused_changes: Vec::new(),
            rules: Some(TeamRulesManifest {
                team: TeamInfo {
                    id: "team".into(),
                    name: None,
                },
                page_id: page,
                file: String::new(),
                project: None,
                skills: Vec::new(),
                proposal_decisions: Default::default(),
            }),
        };
        let result = push(
            &HostLink {
                key: "fake-key".into(),
                url,
                space: "Team".into(),
                space_id: None,
            },
            scratch.path(),
            "member",
            &listed,
            &[],
        )
        .await;
        host.await.unwrap();
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn revoked_key_wins_over_a_concurrent_rules_refusal() {
        let sent = vec![
            Err(SyncError::OutdatedHost(vec!["a".repeat(64)])),
            Err(SyncError::Revoked("the key no longer works".into())),
        ];
        assert!(matches!(upload_failure(sent), Some(SyncError::Revoked(_))));
    }

    #[test]
    fn only_private_addresses_may_be_plain_http() {
        for ok in [
            "https://wenlan.example.com",
            "https://wenlan.example.com/",
            "http://127.0.0.1:7879",
            "http://192.168.1.20:7879",
            "http://10.0.0.5:7879",
            "http://100.101.102.103:7879",
            "http://[fd7a:115c:a1e0::1]:7879",
            "http://server.tail1234.ts.net:7879",
            "http://nas:7879",
            "http://studio.local:7879",
        ] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for refused in [
            "http://wenlan.example.com",
            "http://8.8.8.8:7879",
            "ftp://wenlan.example.com",
            "https://user:pw@wenlan.example.com",
            "https://wenlan.example.com/?x=1",
            "not a url",
        ] {
            assert!(check_url(refused).is_err(), "{refused}");
        }
        assert_eq!(
            check_url("https://wenlan.example.com/").unwrap(),
            "https://wenlan.example.com"
        );
    }

    #[test]
    fn space_names_are_one_path_segment() {
        assert_eq!(
            space_url("https://h", "婚禮 準備/x", "/files"),
            "https://h/team/v1/spaces/%E5%A9%9A%E7%A6%AE%20%E6%BA%96%E5%82%99%2Fx/files"
        );
    }

    #[test]
    fn dot_segments_are_encoded_before_they_can_become_path_segments() {
        for id in ["", ".", ".."] {
            assert!(urlencoding_component(id).is_err(), "{id:?}");
        }
        assert_eq!(urlencoding_component("page/x").unwrap(), "page%2Fx");
    }
}
