// SPDX-License-Identifier: Apache-2.0
//! Validate the exact skill text and file paths shown in a personal proposal.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context, Result};
use unicode_normalization::UnicodeNormalization as _;
use wenlan_types::rulesync_skill::{
    SaveSkillRequest, MAX_SAVED_SKILL_BYTES, MAX_SAVED_SKILL_FILES, MAX_SAVED_SKILL_FILE_BYTES,
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
    let mut paths = BTreeSet::new();
    let mut content = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        let path = portable_path(path)?;
        let collision: String = path.nfkc().flat_map(char::to_lowercase).collect();
        ensure!(
            paths.insert(collision),
            "skill asset paths collide on another file system"
        );
        content.push(SkillFile {
            path,
            bytes: bytes.clone(),
        });
    }
    for path in &paths {
        ensure!(
            path.match_indices('/')
                .all(|(index, _)| !paths.contains(&path[..index])),
            "a skill asset path is also used as a directory"
        );
    }
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
        files.push(SkillPreviewFile {
            path: file.path,
            text: file.text,
            cut: file.cut,
            marked: 0,
        });
    }
    files.sort_by(|left, right| {
        (left.path != "SKILL.md", &left.path).cmp(&(right.path != "SKILL.md", &right.path))
    });
    Ok(files)
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
}
