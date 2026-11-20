// SPDX-License-Identifier: Apache-2.0
//! Validate the exact skill text and file paths shown in a personal proposal.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization as _;
use wenlan_types::rulesync_skill::{
    OversizedSkillFile, SaveSkillRequest, SkillFileDigest, MAX_SAVED_SKILL_BYTES,
    MAX_SAVED_SKILL_FILES, MAX_SAVED_SKILL_FILE_BYTES,
};
use wenlan_types::skill_registry::SkillPreviewFile;

use crate::skill_registry::{self, SkillFile};

pub type SkillFiles = BTreeMap<PathBuf, Vec<u8>>;

pub fn skill_name(name: &str) -> bool {
    skill_registry::folder_name("", name).as_deref() == Some(name) && portable_component(name)
}

fn portable_component(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.ends_with(['.', ' '])
        && !name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        && !name
            .chars()
            .any(|c| c.is_control() || skill_registry::is_invisible(c))
        && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !port.is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn portable_path(path: &Path) -> Result<String> {
    let parts = path
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str().filter(|name| portable_component(name)),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .context("a skill asset must have a safe relative path")?;
    ensure!(
        !parts.is_empty() && parts.len() <= 16,
        "a skill asset path has too many or no components"
    );
    Ok(parts.join("/"))
}

pub fn uploaded_files(request: &SaveSkillRequest) -> Result<SkillFiles> {
    ensure!(
        skill_name(&request.name),
        "use a skill name with lowercase letters, digits, and dashes"
    );
    ensure!(
        !request.description.trim().is_empty() && !request.description.contains(['\n', '\r', '\t']),
        "write a description on one line"
    );
    ensure!(
        skill_registry::printable(&request.description) == request.description,
        "the description holds invisible or control characters"
    );
    ensure!(
        !request.body.trim().is_empty(),
        "write the skill's Markdown body"
    );
    let description = serde_json::to_string(&request.description)?;
    let skill_md = format!(
        "---\nname: {}\ndescription: {description}\n---\n{}",
        request.name, request.body
    );
    let mut files = BTreeMap::from([(PathBuf::from("SKILL.md"), skill_md.into_bytes())]);
    for file in &request.files {
        let path = PathBuf::from(&file.path);
        // Require the portable spelling on the wire too. PathBuf normalizes
        // repeated separators and dot components on some hosts.
        ensure!(
            portable_path(&path)? == file.path,
            "use a clean relative path for each skill asset"
        );
        ensure!(
            files.insert(path, file.text.as_bytes().to_vec()).is_none(),
            "a skill asset path is repeated"
        );
    }
    preview(&request.name, &files)?;
    Ok(files)
}

#[derive(Default)]
pub struct ManagedSkillPreview {
    pub files: Vec<SkillPreviewFile>,
    pub oversized: Vec<OversizedSkillFile>,
}

/// Preview just the changed files in a managed skill. The full snapshot stays
/// available to acceptance, but unchanged assets are not shown to reviewers.
pub fn preview_changes(
    name: &str,
    files: &SkillFiles,
    previous: &SkillFiles,
    changed_paths: &BTreeSet<PathBuf>,
    removed_paths: &BTreeSet<PathBuf>,
) -> Result<ManagedSkillPreview> {
    ensure!(skill_name(name), "the skill folder name is not portable");
    ensure!(
        changed_paths.len() + removed_paths.len() <= MAX_SAVED_SKILL_FILES,
        "the skill has too many changed files"
    );
    for path in removed_paths {
        portable_path(path)?;
        ensure!(
            !files.contains_key(path),
            "a removed skill asset is still present"
        );
    }
    ensure!(
        changed_paths.iter().all(|path| files.contains_key(path)),
        "a changed skill asset is missing"
    );
    // Compare names across the package so a changed path cannot collide with
    // an unchanged asset on case-insensitive file systems. Adopted packages
    // may already include dotfiles that the save API does not accept.
    check_path_collisions(files.keys().map(|path| {
        path.iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }))?;
    let mut preview = ManagedSkillPreview::default();
    let mut text_bytes = 0;
    for path in changed_paths {
        let bytes = &files[path];
        let before = previous.get(path);
        let path = portable_path(path)?;
        let text = std::str::from_utf8(bytes).ok();
        ensure!(
            text.is_some() || !skill_registry::must_be_readable_text(&path),
            "{path} is not readable text (not valid UTF-8)"
        );
        if let Some(text) = text {
            // Existing marked lines may survive an unrelated edit. Match
            // them in order and by exact text, including repeated occurrences.
            let mut previous_lines = marked_lines(
                before
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .unwrap_or_default(),
            );
            ensure!(
                marked_lines(text).all(|line| previous_lines.any(|old| old == line)),
                "{path} introduces invisible or control characters on a changed line"
            );
        }
        if bytes.len() > MAX_SAVED_SKILL_FILE_BYTES
            || text.is_some_and(|text| text.chars().nth(skill_registry::PREVIEW_CHARS).is_some())
        {
            preview.oversized.push(OversizedSkillFile {
                path,
                before: before.map(|bytes| file_digest(bytes)),
                after: file_digest(bytes),
            });
            continue;
        }
        text_bytes += bytes.len();
        ensure!(
            text_bytes <= MAX_SAVED_SKILL_BYTES,
            "the skill preview exceeds the size limit"
        );
        let content = SkillFile {
            path,
            bytes: bytes.clone(),
        };
        preview.files.extend(
            skill_registry::preview_files(&[content])
                .into_iter()
                .map(preview_file),
        );
    }
    sort_preview(&mut preview.files);
    Ok(preview)
}

fn marked_lines(text: &str) -> impl Iterator<Item = &str> {
    skill_registry::without_leading_bom(text)
        .split_inclusive('\n')
        .filter(|line| {
            let line = line.strip_suffix("\r\n").unwrap_or(line);
            line.chars().any(skill_registry::needs_a_mark)
        })
}

fn file_digest(bytes: &[u8]) -> SkillFileDigest {
    SkillFileDigest {
        bytes: bytes.len(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

pub fn preview(name: &str, files: &SkillFiles) -> Result<Vec<SkillPreviewFile>> {
    ensure!(skill_name(name), "the skill folder name is not portable");
    ensure!(
        files.contains_key(Path::new("SKILL.md")),
        "the skill needs a regular SKILL.md"
    );
    ensure!(
        files.len() <= MAX_SAVED_SKILL_FILES,
        "the skill has too many files"
    );
    ensure!(
        files
            .values()
            .all(|bytes| bytes.len() <= MAX_SAVED_SKILL_FILE_BYTES)
            && files.values().map(Vec::len).sum::<usize>() <= MAX_SAVED_SKILL_BYTES,
        "the skill exceeds the size limit"
    );
    let mut content = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        content.push(SkillFile {
            path: portable_path(path)?,
            bytes: bytes.clone(),
        });
    }
    check_path_collisions(content.iter().map(|file| &file.path))?;
    if let Some(reason) = skill_registry::hidden_from_the_person(&content) {
        anyhow::bail!("{reason}");
    }
    let mut files = Vec::with_capacity(content.len());
    for file in skill_registry::preview_files(&content) {
        ensure!(
            file.marked == 0,
            "{} holds invisible or control characters",
            file.path
        );
        // Tool pull-back may contain non-text assets. The client sees that
        // explicitly and can require a separate inspection before approval.
        files.push(preview_file(file));
    }
    sort_preview(&mut files);
    Ok(files)
}

fn preview_file(file: skill_registry::ShownFile) -> SkillPreviewFile {
    SkillPreviewFile {
        path: file.path,
        text: file.text,
        cut: file.cut,
        marked: file.marked as u32,
    }
}

fn sort_preview(files: &mut [SkillPreviewFile]) {
    files.sort_by(|left, right| {
        (left.path != "SKILL.md", &left.path).cmp(&(right.path != "SKILL.md", &right.path))
    });
}

fn check_path_collisions(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Result<()> {
    let mut normalized = BTreeSet::new();
    for path in paths {
        let key: String = path.as_ref().nfkc().flat_map(char::to_lowercase).collect();
        ensure!(
            normalized.insert(key),
            "skill asset paths collide on another file system"
        );
    }
    for path in &normalized {
        ensure!(
            path.match_indices('/')
                .all(|(index, _)| !normalized.contains(&path[..index])),
            "a skill asset path is also used as a directory"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::rulesync_skill::{SavedSkillFile, SavedSkillScope};

    fn request() -> SaveSkillRequest {
        SaveSkillRequest {
            name: "review-notes".into(),
            description: "Handle A: \"quoted\" wording".into(),
            body: "Read the notes.\n".into(),
            files: vec![SavedSkillFile {
                path: "examples/note.md".into(),
                text: "Example text.\n".into(),
            }],
            scope: SavedSkillScope::Shared,
        }
    }

    #[test]
    fn rulesync_skill_approves_exact_visible_files_and_escapes_metadata() {
        let request = request();
        let files = uploaded_files(&request).unwrap();
        let markdown = std::str::from_utf8(&files[Path::new("SKILL.md")]).unwrap();
        assert_eq!(
            skill_registry::front_matter_field(markdown, "description"),
            Some(request.description)
        );
        assert_eq!(
            preview(&request.name, &files).unwrap()[0].text.as_deref(),
            Some(markdown)
        );
        assert_eq!(files[Path::new("examples/note.md")], b"Example text.\n");
    }

    #[test]
    fn rulesync_skill_refuses_invisible_text_and_nonportable_assets() {
        for text in ["x\u{202e}y", "x\u{200b}y", "x\u{001b}y"] {
            let mut body = request();
            body.body = text.into();
            assert!(uploaded_files(&body).is_err());
            let mut description = request();
            description.description = text.into();
            assert!(uploaded_files(&description).is_err());
            let mut asset = request();
            asset.files[0].text = text.into();
            assert!(uploaded_files(&asset).is_err());
        }
        for path in [
            "../escape",
            "/absolute",
            "assets//x",
            "assets/./x",
            ".git/config",
            "C:\\x",
            "con.md",
            "skill.md",
            "SKILL.md/subfile",
        ] {
            let mut request = request();
            request.files[0].path = path.into();
            assert!(uploaded_files(&request).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn rulesync_skill_delta_preview_allows_asset_only_changes_and_deletions() {
        let mut files = BTreeMap::from([
            (
                PathBuf::from("SKILL.md"),
                b"Stable instructions.\n".to_vec(),
            ),
            (
                PathBuf::from("assets/new.md"),
                b"New reviewed asset.\n".to_vec(),
            ),
            (PathBuf::from(".gitignore"), b"scratch/\n".to_vec()),
        ]);
        let changed = BTreeSet::from([PathBuf::from("assets/new.md")]);
        let removed = BTreeSet::from([PathBuf::from("assets/old.md")]);
        let previous = SkillFiles::new();
        let preview = preview_changes("review-notes", &files, &previous, &changed, &removed)
            .unwrap()
            .files;
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].path, "assets/new.md");
        assert_eq!(preview[0].text.as_deref(), Some("New reviewed asset.\n"));

        let no_changed_files = BTreeSet::new();
        let preview = preview_changes(
            "review-notes",
            &files,
            &previous,
            &no_changed_files,
            &removed,
        )
        .unwrap()
        .files;
        assert!(preview.is_empty());

        files.insert(PathBuf::from("assets/NEW.md"), Vec::new());
        assert!(preview_changes("review-notes", &files, &previous, &changed, &removed).is_err());
    }
}
