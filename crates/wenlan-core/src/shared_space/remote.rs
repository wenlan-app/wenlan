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
//! fetched or sent once. The server takes only this member's own versions,
//! and none from a member who may only read.
//!
//! Plain `http://` is used only for an address that does not leave a
//! private network (this computer, a LAN, Tailscale); anything else must be
//! `https://`: the member key goes with every call.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use wenlan_types::team::{TeamAccess, TeamFileList, TeamMe};

use super::{version, HostLink, SPACE_FILE};
use crate::error::WenlanError;

/// This Wenlan's copies of team servers' folders, beside the settings file.
pub const MIRROR_DIR: &str = "team-mirror";

const TIMEOUT: Duration = Duration::from_secs(30);
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

fn space_url(url: &str, space: &str, rest: &str) -> String {
    let mut out = format!("{url}/team/v1/spaces/");
    out.push_str(&urlencoding_component(space));
    out.push_str(rest);
    out
}

/// Percent-encodes one path segment.
fn urlencoding_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
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

/// Who `key` belongs to on the server at `url`.
pub async fn me(url: &str, key: &str) -> Result<TeamMe, String> {
    let url = check_url(url)?;
    let response = client()?
        .get(format!("{url}/team/v1/me"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|error| unreachable(&url, error))?;
    if !response.status().is_success() {
        return Err(refused(&url, response.status()));
    }
    response
        .json::<TeamMe>()
        .await
        .map_err(|_| format!("{url} is not a Wenlan team server"))
}

/// Copies every file of the server's folder that `folder` does not have
/// yet, and the server's `space.json`. Returns the server's file list.
pub async fn pull(link: &HostLink, folder: &Path) -> Result<TeamFileList, String> {
    let url = check_url(&link.url)?;
    let client = client()?;
    let response = client
        .get(space_url(&url, &link.space, "/files"))
        .bearer_auth(&link.key)
        .send()
        .await
        .map_err(|error| unreachable(&url, error))?;
    if !response.status().is_success() {
        return Err(refused(&url, response.status()));
    }
    let list: TeamFileList = response
        .json()
        .await
        .map_err(|_| format!("{url} is not a Wenlan team server"))?;
    if !list.files.iter().any(|path| path == SPACE_FILE) {
        return Err("the team server has not shared this Space yet".into());
    }
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;

    let wanted: Vec<(String, PathBuf)> = list
        .files
        .iter()
        .filter_map(|path| Some((path.clone(), crate::team::served_path(folder, path)?)))
        .filter(|(path, local)| path == SPACE_FILE || !version::version_file_exists(local))
        .collect();
    let fetched: Vec<Result<(), String>> = futures::stream::iter(wanted)
        .map(|(path, local)| {
            let client = client.clone();
            let url = url.clone();
            async move {
                let response = client
                    .get(space_url(&url, &link.space, "/file"))
                    .query(&[("path", path.as_str())])
                    .bearer_auth(&link.key)
                    .send()
                    .await
                    .map_err(|error| unreachable(&url, error))?;
                if !response.status().is_success() {
                    return Err(refused(&url, response.status()));
                }
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|error| unreachable(&url, error))?;
                if bytes.len() > MAX_VERSION_BYTES {
                    return Err(format!("{path} is too large"));
                }
                tokio::task::spawn_blocking(move || store(&path, &local, &bytes))
                    .await
                    .map_err(|error| error.to_string())?
            }
        })
        .buffer_unordered(PARALLEL)
        .collect()
        .await;
    for result in fetched {
        result?;
    }
    Ok(list)
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
/// Returns how many were sent. A member who may only read sends none.
pub async fn push(
    link: &HostLink,
    folder: &Path,
    member: &str,
    listed: &TeamFileList,
) -> Result<u32, String> {
    if listed.access == TeamAccess::Read {
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
        .filter(|(path, ..)| !have.contains(version::version_key(path)))
        .collect();
    let sent: Vec<Result<(), String>> = futures::stream::iter(missing)
        .map(|(path, page, local)| {
            let client = client.clone();
            let url = url.clone();
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
                    // A version the server cannot take as it is: sending it
                    // again would not change that.
                    log::warn!("[shared_space] the team server did not take {path}");
                    Ok(())
                } else {
                    Err(refused(&url, response.status()))
                }
            }
        })
        .buffer_unordered(PARALLEL)
        .collect()
        .await;
    let mut count = 0;
    for result in sent {
        result?;
        count += 1;
    }
    Ok(count)
}

/// This member's version files in the copy: (path as the server lists it,
/// page id, file).
fn own_versions(folder: &Path, member: &str) -> Vec<(String, String, PathBuf)> {
    let pages = folder
        .join(version::MEMBERS_DIR)
        .join(member)
        .join(version::PAGES_DIR);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&pages) else {
        return out;
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
                    format!(
                        "{}/{member}/{}/{page_id}/{name}",
                        version::MEMBERS_DIR,
                        version::PAGES_DIR
                    ),
                    page_id.clone(),
                    file.path(),
                ));
            }
        }
    }
    out.sort();
    out
}

/// Turns a refusal to read or write into an error for a route.
pub fn as_error(reason: String) -> WenlanError {
    WenlanError::Validation(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
