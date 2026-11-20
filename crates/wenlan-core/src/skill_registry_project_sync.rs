// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;

use wenlan_types::team::{TeamProjectSkill, TeamProjectSkillFile};

use super::*;
use crate::rulesync_personal::reject_secret_lines;
use crate::store::format::{render_frontmatter, split_frontmatter};

const MAX_SKILLS: usize = 32;
const MAX_FILES_PER_SKILL: usize = 32;
const MAX_TEXT: usize = 64 * 1024;
const MAX_BUNDLE_BYTES: usize = 512 * 1024;

/// Read only the exact Space's installed, hash-checked registry sources.
pub(crate) fn project_sources(root: &Path, space: &str) -> anyhow::Result<Vec<TeamProjectSkill>> {
    let Some(root) = optional_dir(Dir::open_ambient_dir(root, cap_std::ambient_authority()))?
    else {
        return Ok(Vec::new());
    };
    let Some(spaces) = optional_dir(root.open_dir_nofollow("spaces"))? else {
        return Ok(Vec::new());
    };
    let Some(dir) =
        optional_dir(spaces.open_dir_nofollow(crate::space_folders::folder_name(space)))?
    else {
        return Ok(Vec::new());
    };
    let Some(registry) = optional_dir(dir.open_dir_nofollow("skills"))? else {
        return Ok(Vec::new());
    };
    let scope = Scope::Space {
        name: space.to_string(),
    };
    let mut names = BTreeSet::new();
    for entry in registry.entries()? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry
                .file_name()
                .to_str()
                .filter(|name| safe_skill_name(name))
            {
                names.insert(name.to_string());
            }
        }
    }
    let mut skills = Vec::new();
    let mut total = 0;
    for name in names {
        let candidate = match snapshot(&registry, &scope, &name) {
            Ok(Some(candidate)) => candidate,
            Ok(None) => continue,
            Err(_) => {
                log::warn!("[team Rules] an installed project skill did not match its approved source or could not be read");
                continue;
            }
        };
        let Ok(size) = validate_skill(&candidate) else {
            log::warn!("[team Rules] a project skill was omitted because it is outside the shared text format");
            continue;
        };
        if skills.len() == MAX_SKILLS || total + size > MAX_BUNDLE_BYTES {
            log::warn!(
                "[team Rules] a project skill was omitted because the shared bundle is full"
            );
            continue;
        }
        total += size;
        skills.push(candidate);
    }
    Ok(skills)
}

fn snapshot(registry: &Dir, scope: &Scope, name: &str) -> anyhow::Result<Option<TeamProjectSkill>> {
    let skill = registry.open_dir_nofollow(name)?;
    let Some(control) = optional_dir(skill.open_dir_nofollow(META_DIR))? else {
        return Ok(None);
    };
    let metadata = open_regular(&control, RECORD_FILE)?;
    ensure!(
        metadata.metadata()?.len() <= MAX_TEXT as u64,
        "the project skill registry record is too large"
    );
    let record: InstalledSkill = serde_json::from_reader(metadata.take((MAX_TEXT + 1) as u64))?;
    if record.scope != *scope || record.name != name || record.verdict == Verdict::Fail {
        return Ok(None);
    }
    let mut files = Vec::new();
    let mut total = 0;
    read_personal_author(
        &control.open_dir_nofollow(AUTHOR_DIR)?,
        "",
        &mut files,
        &mut total,
    )?;
    ensure!(
        hash_files(&files) == record.hash,
        "the approved project skill files changed"
    );
    ensure!(
        hidden_from_the_person(&files).is_none() && runnable_files(&files).is_empty(),
        "the project skill contains concealed or runnable files"
    );
    let mut additions = String::new();
    open_regular(&control, ADDITIONS_FILE)?
        .take((MAX_TEXT + 1) as u64)
        .read_to_string(&mut additions)?;
    ensure!(
        additions.len() <= MAX_TEXT,
        "the project skill additions exceed the text limit"
    );
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let files = files
        .into_iter()
        .map(|file| {
            let text = String::from_utf8(file.bytes)?;
            let text = if file.path == "SKILL.md" {
                let approved = canonical_skill_text(&text, name)?;
                compose_skill_md(&approved, scope, &additions)
            } else {
                text
            };
            Ok(TeamProjectSkillFile {
                path: file.path,
                text,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Some(TeamProjectSkill {
        name: name.to_string(),
        files,
    }))
}

/// Carry name and description only. Renderer metadata may change how a tool runs.
fn canonical_skill_text(text: &str, name: &str) -> anyhow::Result<String> {
    ensure!(text.len() <= MAX_TEXT, "the project skill is too large");
    let text = without_leading_bom(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines
        .next()
        .context("the project skill needs frontmatter")?;
    ensure!(
        matches!(first, "---\n" | "---\r\n"),
        "the project skill needs frontmatter"
    );
    let mut end = first.len();
    let mut closed = false;
    for line in lines {
        end += line.len();
        if matches!(line, "---\n" | "---\r\n" | "---") {
            closed = true;
            break;
        }
    }
    ensure!(
        closed,
        "the project skill needs a closing frontmatter fence"
    );
    let (header, body) = text.split_at(end);
    let header = header.replace("\r\n", "\n");
    let metadata = split_frontmatter(&header)?
        .context("the project skill needs frontmatter")?
        .frontmatter;
    ensure!(
        metadata.len() == 2,
        "a project skill can share only its name and description in frontmatter"
    );
    ensure!(
        metadata.get("name").and_then(serde_yaml::Value::as_str) == Some(name),
        "the project skill name must match its registry name"
    );
    let description = metadata
        .get("description")
        .and_then(serde_yaml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("the project skill needs a plain text description")?;
    let mut plain = serde_yaml::Mapping::new();
    plain.insert("name".into(), name.into());
    plain.insert("description".into(), description.into());
    let mut output = render_frontmatter(&plain);
    output.push_str(body);
    Ok(output)
}

fn safe_skill_name(name: &str) -> bool {
    local_name(Path::new(name)).as_deref() == Some(name) && portable_component(name)
}

fn portable_component(component: &str) -> bool {
    if component.is_empty()
        || component.starts_with('.')
        || component.ends_with('.')
        || !component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return false;
    }
    let base = component
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit())
}

fn safe_text_path(path: &str) -> bool {
    path.len() <= 240
        && path.split('/').count() <= 8
        && path.split('/').all(portable_component)
        && path.rsplit_once('.').is_some_and(|(_, extension)| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("txt")
        })
}

pub(crate) fn validate(skills: &[TeamProjectSkill]) -> anyhow::Result<()> {
    ensure!(skills.len() <= MAX_SKILLS, "too many project skills");
    let mut names = BTreeSet::new();
    let mut total = 0;
    for skill in skills {
        ensure!(
            names.insert(skill.name.as_str()),
            "duplicate project skill name"
        );
        total += validate_skill(skill)?;
        ensure!(
            total <= MAX_BUNDLE_BYTES,
            "project skills exceed the shared bundle size"
        );
    }
    Ok(())
}

fn validate_skill(skill: &TeamProjectSkill) -> anyhow::Result<usize> {
    ensure!(safe_skill_name(&skill.name), "invalid project skill name");
    reject_secret_lines(&skill.name)?;
    ensure!(
        skill.files.len() <= MAX_FILES_PER_SKILL,
        "too many project skill files"
    );
    ensure!(
        skill.files.iter().any(|file| file.path == "SKILL.md"),
        "the project skill needs SKILL.md"
    );
    let mut paths = BTreeSet::new();
    let mut total = skill.name.len();
    for file in &skill.files {
        ensure!(
            safe_text_path(&file.path),
            "invalid project skill text path"
        );
        reject_secret_lines(&file.path)?;
        let path = file.path.to_ascii_lowercase();
        ensure!(
            path.rsplit('/').next() != Some("skill.md") || file.path == "SKILL.md",
            "a project skill may contain only one top-level SKILL.md"
        );
        ensure!(paths.insert(path), "duplicate project skill text path");
        let visible = without_leading_bom(&file.text);
        ensure!(
            file.text.len() <= MAX_TEXT
                && shown_text(&file.text).1 == 0
                && !visible.starts_with("#!"),
            "invalid project skill text"
        );
        reject_secret_lines(&file.text)?;
        if file.path == "SKILL.md" {
            ensure!(
                canonical_skill_text(&file.text, &skill.name)? == file.text,
                "project skill frontmatter is outside the shared text format"
            );
        }
        total += file.path.len() + file.text.len();
    }
    ensure!(
        paths.iter().all(|path| !path
            .match_indices('/')
            .any(|(at, _)| paths.contains(&path[..at]))),
        "a project skill file is also a directory"
    );
    Ok(total)
}

#[cfg(test)]
pub(crate) fn store_test_skill(root: &Path, scope: Scope, name: &str, mut files: Vec<SkillFile>) {
    if !files.iter().any(|file| file.path == "SKILL.md") {
        files.push(SkillFile {
            path: "SKILL.md".into(),
            bytes: format!(
                "---\nname: {name}\ndescription: A registry fixture\n---\nUse the approved source.\n"
            )
            .into_bytes(),
        });
    }
    let fetched = FetchedSkill {
        source: "fixture/project-skills".into(),
        skill_id: name.to_string(),
        commit: "a".repeat(40),
        repo_path: "".into(),
        hash: hash_files(&files),
        files,
    };
    store_skill(root, &fetched, Verdict::Pass, scope, 0).unwrap();
}
