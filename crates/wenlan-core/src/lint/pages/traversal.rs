use super::frontmatter::{read_frontmatter, Frontmatter};
use super::fs::{EntryKind, EntryScope, PageEntry, PageFsError, PageScanControl};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, Metadata, OpenOptions};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;

pub(crate) fn open_root(root: &Path) -> Result<Dir, PageFsError> {
    let parent = root.parent().ok_or(PageFsError::RootNotDirectory)?;
    let basename = root.file_name().ok_or(PageFsError::RootNotDirectory)?;
    let ambient_parent = Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .map_err(|_| PageFsError::ReadDirectory)?;
    let metadata = ambient_parent
        .symlink_metadata(Path::new(basename))
        .map_err(|_| PageFsError::ReadMetadata)?;
    if metadata.file_type().is_symlink() {
        return Err(PageFsError::RootSymlink);
    }
    if !metadata.is_dir() {
        return Err(PageFsError::RootNotDirectory);
    }
    ambient_parent
        .open_dir_nofollow(Path::new(basename))
        .map_err(|_| PageFsError::ReadDirectory)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn collect_entries(
    root: &Dir,
    entries: &mut Vec<PageEntry>,
    state_bytes: &mut Option<Vec<u8>>,
    manifest_bytes: &mut Option<Vec<u8>>,
    manifest_too_large: &mut bool,
    include_body_digests: bool,
    body_bytes_remaining: &mut u64,
    control: &PageScanControl,
) -> Result<(), PageFsError> {
    let mut traversal = Traversal {
        entries,
        state_bytes,
        manifest_bytes,
        manifest_too_large,
        include_body_digests,
        body_bytes_remaining,
        control,
    };
    visit(root, "", &mut traversal)
}

struct Traversal<'a> {
    entries: &'a mut Vec<PageEntry>,
    state_bytes: &'a mut Option<Vec<u8>>,
    manifest_bytes: &'a mut Option<Vec<u8>>,
    manifest_too_large: &'a mut bool,
    include_body_digests: bool,
    body_bytes_remaining: &'a mut u64,
    control: &'a PageScanControl,
}

fn visit(directory: &Dir, prefix: &str, traversal: &mut Traversal<'_>) -> Result<(), PageFsError> {
    traversal.control.check()?;
    let mut names = directory
        .entries()
        .map_err(|_| PageFsError::ReadDirectory)?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|_| PageFsError::ReadEntry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    for name in names {
        traversal.control.check()?;
        visit_entry(directory, prefix, &name, traversal)?;
    }
    Ok(())
}

fn visit_entry(
    directory: &Dir,
    prefix: &str,
    name: &OsStr,
    traversal: &mut Traversal<'_>,
) -> Result<(), PageFsError> {
    traversal.control.check()?;
    let component = component_string(name)?;
    let path = if prefix.is_empty() {
        component
    } else {
        format!("{prefix}/{component}")
    };
    if is_projection_control_path(&path) {
        return Ok(());
    }
    let metadata = directory
        .symlink_metadata(Path::new(name))
        .map_err(|_| PageFsError::ReadMetadata)?;
    let classified = entry_kind(&metadata);

    match classified {
        EntryKind::Directory => {
            let child = directory
                .open_dir_nofollow(Path::new(name))
                .map_err(|_| PageFsError::ReadDirectory)?;
            let opened = child
                .dir_metadata()
                .map_err(|_| PageFsError::ReadMetadata)?;
            traversal.entries.push(page_entry(
                &path,
                EntryKind::Directory,
                &opened,
                Frontmatter::unparsed(),
                None,
            )?);
            visit(&child, &path, traversal)
        }
        EntryKind::File => {
            let mut file = directory
                .open_with(Path::new(name), &read_nofollow())
                .map_err(|_| PageFsError::ReadEntry)?;
            let opened = file.metadata().map_err(|_| PageFsError::ReadMetadata)?;
            if is_projection_stage_owner_candidate(&path)
                && valid_projection_stage_owner(&path, &opened, &mut file, traversal.control)?
            {
                return Ok(());
            }
            let scope = scope_for(&path, EntryKind::File);
            if path == ".wenlan/state.json" {
                let mut bytes = Vec::new();
                read_to_end_controlled(
                    &mut (&mut file).take(super::fs::STATE_MAX_BYTES + 1),
                    &mut bytes,
                    traversal.control,
                )?;
                if bytes.len() > usize::try_from(super::fs::STATE_MAX_BYTES).unwrap_or(usize::MAX) {
                    return Err(PageFsError::StateBudgetExceeded);
                }
                *traversal.state_bytes = Some(bytes);
            } else if path == "_sources/.manifest.json" {
                let mut bytes = Vec::new();
                read_to_end_controlled(
                    &mut (&mut file).take(super::fs::MANIFEST_MAX_BYTES + 1),
                    &mut bytes,
                    traversal.control,
                )?;
                if bytes.len()
                    > usize::try_from(super::fs::MANIFEST_MAX_BYTES).unwrap_or(usize::MAX)
                {
                    *traversal.manifest_too_large = true;
                } else {
                    *traversal.manifest_bytes = Some(bytes);
                }
            }
            let (frontmatter, body_digest) = if scope == EntryScope::PageMarkdown
                && traversal.include_body_digests
            {
                if opened.len() > super::fs::DEEP_PAGE_BODY_MAX_BYTES
                    || opened.len() > *traversal.body_bytes_remaining
                {
                    return Err(PageFsError::BodyBudgetExceeded);
                }
                *traversal.body_bytes_remaining =
                    (*traversal.body_bytes_remaining).saturating_sub(opened.len());
                let mut bytes = Vec::new();
                read_to_end_controlled(&mut file, &mut bytes, traversal.control)?;
                let frontmatter = super::frontmatter::parse_frontmatter(bytes.as_slice())?;
                let body_digest = match std::str::from_utf8(&bytes) {
                    Ok(content) => {
                        // The watcher's decoder and save-noise rule: a body
                        // that starts with blank lines or was only re-saved
                        // by an editor is not drift; a real edit is.
                        use crate::export::provenance::{page_body_from_projection, page_text_key};
                        let body = page_body_from_projection(content);
                        Sha256::digest(page_text_key(&body).as_bytes()).into()
                    }
                    Err(_) => {
                        let mut digest = Sha256::new();
                        digest.update(b"wenlan-page-non-utf8-body-v1");
                        digest.update(&bytes);
                        digest.finalize().into()
                    }
                };
                (frontmatter, Some(body_digest))
            } else if scope == EntryScope::PageMarkdown {
                let frontmatter = read_frontmatter(file)?;
                traversal.control.check()?;
                (frontmatter, None)
            } else {
                (Frontmatter::unparsed(), None)
            };
            traversal.entries.push(page_entry(
                &path,
                EntryKind::File,
                &opened,
                frontmatter,
                body_digest,
            )?);
            Ok(())
        }
        EntryKind::Symlink | EntryKind::Other => {
            traversal.entries.push(special_entry(
                directory, name, &path, classified, &metadata,
            )?);
            Ok(())
        }
    }
}

fn is_projection_control_path(path: &str) -> bool {
    path == ".wenlan/.projection.lock"
        || path
            .strip_prefix(".wenlan/.projection-state-")
            .is_some_and(|suffix| suffix.ends_with(".tmp"))
}

fn is_projection_stage_owner_candidate(path: &str) -> bool {
    projection_stage_owner_hash(path).is_some()
}

fn projection_stage_owner_hash(path: &str) -> Option<&str> {
    path.strip_prefix(".wenlan/.projection-unlink-")
        .and_then(|suffix| suffix.strip_suffix("/owner.json"))
        .filter(|owner| {
            owner.len() == 64
                && owner
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectionStageOwnerMarker {
    format_version: u32,
    manifest_id: String,
    page_id: String,
    source_path: String,
    source_digest: String,
}

fn valid_projection_stage_owner(
    path: &str,
    metadata: &Metadata,
    file: &mut cap_std::fs::File,
    control: &PageScanControl,
) -> Result<bool, PageFsError> {
    const OWNER_MAX_BYTES: u64 = 16 * 1024;

    let Some(expected_stage_hash) = projection_stage_owner_hash(path) else {
        return Ok(false);
    };
    if metadata.len() > OWNER_MAX_BYTES {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    read_to_end_controlled(
        &mut file.take(OWNER_MAX_BYTES.saturating_add(1)),
        &mut bytes,
        control,
    )?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > OWNER_MAX_BYTES {
        return Ok(false);
    }
    let Ok(owner) = serde_json::from_slice::<ProjectionStageOwnerMarker>(&bytes) else {
        return Ok(false);
    };
    let source_path_is_canonical = super::fs::normalize_target_path(&owner.source_path)
        .is_ok_and(|normalized| normalized.as_str() == owner.source_path);
    let source_digest_is_canonical = owner.source_digest.len() == 64
        && owner
            .source_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    let actual_stage_hash = hex::encode(Sha256::digest(owner.manifest_id.as_bytes()));
    Ok(owner.format_version == 1
        && !owner.manifest_id.is_empty()
        && !owner.page_id.is_empty()
        && source_path_is_canonical
        && source_digest_is_canonical
        && actual_stage_hash == expected_stage_hash)
}

fn read_to_end_controlled(
    reader: &mut impl Read,
    output: &mut Vec<u8>,
    control: &PageScanControl,
) -> Result<(), PageFsError> {
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        control.check()?;
        let read = reader
            .read(&mut chunk)
            .map_err(|_| PageFsError::ReadPrefix)?;
        if read == 0 {
            return Ok(());
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

fn special_entry(
    directory: &Dir,
    name: &OsStr,
    path: &str,
    kind: EntryKind,
    metadata: &Metadata,
) -> Result<PageEntry, PageFsError> {
    let modified_ns = modified_ns(metadata)?;
    let prefix_digest = if kind == EntryKind::Symlink {
        let target = directory
            .read_link_contents(Path::new(name))
            .map_err(|_| PageFsError::ReadMetadata)?;
        digest_parts(&[
            b"wenlan-page-symlink-v1",
            target.as_os_str().as_encoded_bytes(),
        ])
    } else {
        digest_parts(&[
            b"wenlan-page-special-v1",
            &metadata.len().to_le_bytes(),
            &modified_ns.to_le_bytes(),
        ])
    };
    Ok(PageEntry {
        path: path.to_string(),
        kind,
        scope: scope_for(path, kind),
        frontmatter: Frontmatter::unparsed(),
        length: metadata.len(),
        modified_ns,
        prefix_digest,
        body_digest: None,
    })
}

fn digest_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part);
    }
    digest.finalize().into()
}

fn read_nofollow() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    options
}

fn page_entry(
    path: &str,
    kind: EntryKind,
    metadata: &Metadata,
    frontmatter: Frontmatter,
    body_digest: Option<[u8; 32]>,
) -> Result<PageEntry, PageFsError> {
    Ok(PageEntry {
        path: path.to_string(),
        kind,
        scope: scope_for(path, kind),
        prefix_digest: frontmatter.digest(),
        body_digest,
        frontmatter,
        length: metadata.len(),
        modified_ns: modified_ns(metadata)?,
    })
}

fn entry_kind(metadata: &Metadata) -> EntryKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_file() {
        EntryKind::File
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    }
}

/// Whether `path` names a file OKF reserves. Spec 3.1 reserves `index.md` and
/// `log.md` at EVERY level of the hierarchy, not only at the bundle root, so
/// this matches on the basename. Case-insensitive, like the default macOS and
/// Windows filesystems.
fn is_reserved_okf_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.eq_ignore_ascii_case("index.md") || name.eq_ignore_ascii_case("log.md")
}

fn scope_for(path: &str, kind: EntryKind) -> EntryScope {
    let first = path.split('/').next().unwrap_or_default();
    if first == ".wenlan" {
        EntryScope::StateControl
    } else if first == "_sources" {
        EntryScope::SourceInventory
    } else if kind == EntryKind::File && is_reserved_okf_path(path) {
        // The names OKF reserves at every level of the hierarchy (spec 3.1):
        // `index.md`, the bundle index Wenlan generates, and `log.md`, a
        // directory's update history. Neither is a concept document, neither
        // carries an `origin_id`, and neither may be scanned as a page.
        EntryScope::Other
    } else if kind == EntryKind::File && path.to_ascii_lowercase().ends_with(".md") {
        EntryScope::PageMarkdown
    } else {
        EntryScope::Other
    }
}

pub(crate) fn relative_path_string(relative: &Path) -> Result<String, PageFsError> {
    relative
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or(PageFsError::UnsupportedFilenameEncoding)
}

fn component_string(name: &OsStr) -> Result<String, PageFsError> {
    relative_path_string(Path::new(name))
}

fn modified_ns(metadata: &Metadata) -> Result<u128, PageFsError> {
    metadata
        .modified()
        .map_err(|_| PageFsError::ReadMetadata)
        .and_then(|time| {
            time.into_std()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .map_err(|_| PageFsError::ReadMetadata)
        })
}
