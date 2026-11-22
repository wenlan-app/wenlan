// SPDX-License-Identifier: AGPL-3.0-only
//! Personal cloud: the person's whole library, kept as files.
//!
//! A person's Wenlan in the personal cloud holds a copy of their library
//! folder: the same `space.json` and `members/<member>/<kind>/<id>/<sha>.wlv`
//! files their computers write into a Google Drive or iCloud folder, here
//! under `cloud-library/` in its data folder. Their computers list, fetch and
//! upload those files; the cloud copy never joins as a computer and runs
//! nothing on them. A version file is kept only when its bytes match the
//! name; one already kept is never replaced.

use std::path::{Path, PathBuf};

use wenlan_core::shared_space::version;

/// The folder, beside `team.json`, that holds the library copy.
pub const LIBRARY_DIR: &str = "cloud-library";
/// The library's own file: its computers and which one is main.
pub const SPACE_FILE: &str = "space.json";
/// The largest file taken: the largest page version a team server takes.
pub const MAX_FILE_BYTES: usize = wenlan_core::shared_space::remote::MAX_VERSION_BYTES;

/// The kinds of version files a library holds.
fn kind_allowed(kind: &str) -> bool {
    version::KIND_DIRS.contains(&kind) || kind == version::MEMORIES_DIR
}

/// Where `path` (as listed) lives under `root`, when it is a path a library
/// holds: `space.json`, or `members/<member>/<kind>/<id>/<sha>.wlv`. None for
/// anything else, so no path can name a file outside the copy.
pub fn local_path(root: &Path, path: &str) -> Option<PathBuf> {
    if path == SPACE_FILE {
        return Some(root.join(SPACE_FILE));
    }
    match path.split('/').collect::<Vec<_>>().as_slice() {
        [members, member, kind, id, name]
            if *members == version::MEMBERS_DIR
                && kind_allowed(kind)
                && version::is_safe_id(member)
                && version::is_safe_id(id)
                && version::version_sha(name).is_some() =>
        {
            Some(
                root.join(members)
                    .join(member)
                    .join(kind)
                    .join(id)
                    .join(name),
            )
        }
        _ => None,
    }
}

/// Every file of the copy, as paths [`local_path`] takes, sorted.
pub fn list(root: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    if root.join(SPACE_FILE).is_file() {
        out.push(SPACE_FILE.to_string());
    }
    let members = root.join(version::MEMBERS_DIR);
    let entries = match std::fs::read_dir(&members) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(error),
    };
    for member in entries.flatten() {
        let Ok(member_name) = member.file_name().into_string() else {
            continue;
        };
        for kind in std::fs::read_dir(member.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(kind_name) = kind.file_name().into_string() else {
                continue;
            };
            for id in std::fs::read_dir(kind.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let Ok(id_name) = id.file_name().into_string() else {
                    continue;
                };
                for file in std::fs::read_dir(id.path()).into_iter().flatten().flatten() {
                    let Ok(file_name) = file.file_name().into_string() else {
                        continue;
                    };
                    let path = format!(
                        "{}/{member_name}/{kind_name}/{id_name}/{file_name}",
                        version::MEMBERS_DIR
                    );
                    if local_path(root, &path).is_some() {
                        out.push(path);
                    }
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Why a file was not kept.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a path a library holds.
    Path,
    /// Larger than [`MAX_FILE_BYTES`].
    TooLarge,
    /// A version file whose bytes do not match its name.
    Mismatch,
    /// `space.json` that is not JSON.
    NotJson,
}

/// Keeps one uploaded file. `space.json` is replaced; a version file is
/// written once and an upload of one already kept changes nothing.
pub fn put(root: &Path, path: &str, bytes: &[u8]) -> Result<Result<(), Refused>, std::io::Error> {
    use std::io::Write;
    let Some(local) = local_path(root, path) else {
        return Ok(Err(Refused::Path));
    };
    if bytes.len() > MAX_FILE_BYTES {
        return Ok(Err(Refused::TooLarge));
    }
    if path == SPACE_FILE {
        if serde_json::from_slice::<serde_json::Value>(bytes).is_err() {
            return Ok(Err(Refused::NotJson));
        }
    } else {
        let name = local.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if version::version_sha(name) != Some(version::sha256_hex(bytes).as_str()) {
            return Ok(Err(Refused::Mismatch));
        }
        if local.exists() {
            return Ok(Ok(()));
        }
    }
    let dir = local.parent().unwrap_or(root);
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".writing-")
        .tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    if path == SPACE_FILE {
        tmp.persist(&local).map_err(|error| error.error)?;
    } else if let Err(error) = tmp.persist_noclobber(&local) {
        if !local.exists() {
            return Err(error.error);
        }
    }
    Ok(Ok(()))
}

#[cfg(test)]
#[path = "cloud_library_test.rs"]
mod tests;
