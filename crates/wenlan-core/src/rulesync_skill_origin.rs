// SPDX-License-Identifier: Apache-2.0
//! Installer metadata for a skill already inside the selected home.

use std::path::{Component, Path};
use std::process::{Command, Stdio};

use serde_json::Value;
use wenlan_types::rulesync::SkillUpstream;

use super::Home;
use crate::{repo_wiki, skill_registry};

pub(super) fn discover(home: &Home, skill: &Path) -> Option<SkillUpstream> {
    skills_lock(home, skill)
        .or_else(|| npm_lock(home, skill))
        .or_else(|| checkout_source(home, skill))
}

fn read_metadata(home: &Home, path: &Path) -> Option<Value> {
    let node = home.resolve(path).ok()??;
    if node.through_link {
        return None;
    }
    serde_json::from_slice(
        &home
            .bytes(&node, 2 * 1024 * 1024, "the installer metadata size limit")
            .ok()?,
    )
    .ok()
}

fn slash_path(path: &Path) -> Option<String> {
    let parts = path
        .components()
        .map(|component| {
            let Component::Normal(part) = component else {
                return None;
            };
            part.to_str()
                .filter(|part| !part.contains(['\\', ':']) && !part.chars().any(char::is_control))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

fn github(remote: &str) -> Option<String> {
    repo_wiki::github_slug(remote).filter(|slug| skill_registry::valid_source(slug))
}

fn skills_lock(home: &Home, skill: &Path) -> Option<SkillUpstream> {
    // The lock belongs to the resolved shared folder. An alias with the
    // same name in another directory does not inherit this metadata.
    if skill.parent()? != Path::new(super::SHARED_SKILLS) {
        return None;
    }
    let lock = read_metadata(home, Path::new(".agents/.skill-lock.json"))?;
    let entry = lock.get("skills")?.get(skill.file_name()?.to_str()?)?;
    if entry.get("sourceType")?.as_str()? != "github" {
        return None;
    }
    let repository = github(entry.get("sourceUrl")?.as_str()?)?;
    if !repository.eq_ignore_ascii_case(entry.get("source")?.as_str()?) {
        return None;
    }
    let skill_md = entry.get("skillPath")?.as_str()?;
    let path = Path::new(skill_md);
    if slash_path(path)? != skill_md || path.file_name()?.to_str()? != "SKILL.md" {
        return None;
    }
    Some(SkillUpstream::GitHub {
        repository,
        directory: slash_path(path.parent()?)?,
        // The npx skillFolderHash is a tree hash, not a commit.
        revision: None,
        checkout: None,
    })
}

fn npm_lock(home: &Home, skill: &Path) -> Option<SkillUpstream> {
    for root in skill.ancestors() {
        let Some(parent) = root.parent() else {
            break;
        };
        let modules = if parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('@'))
        {
            parent.parent()?
        } else {
            parent
        };
        if modules.file_name().and_then(|name| name.to_str()) != Some("node_modules") {
            continue;
        }
        let package = slash_path(root.strip_prefix(modules).ok()?)?;
        let manifest = read_metadata(home, &root.join("package.json"))?;
        let version = manifest.get("version")?.as_str()?;
        if manifest.get("name")?.as_str()? != package || !skill_registry::valid_npm_name(&package) {
            return None;
        }
        let install = modules.parent()?;
        let key = slash_path(root.strip_prefix(install).ok()?)?;
        let lock_paths = [
            install.join("npm-shrinkwrap.json"),
            install.join("package-lock.json"),
            modules.join(".package-lock.json"),
        ];
        let locked = lock_paths.iter().any(|path| {
            read_metadata(home, path).is_some_and(|lock| {
                let record = &lock["packages"][&key];
                record["version"].as_str() == Some(version)
                    && record["resolved"].as_str().is_some_and(|url| {
                        let prefix = format!("https://registry.npmjs.org/{package}/-/");
                        url.starts_with(&prefix) && url.ends_with(&format!("-{version}.tgz"))
                    })
            })
        });
        if locked && skill_registry::valid_npm_version(version) {
            return Some(SkillUpstream::Npm {
                package,
                version: version.to_owned(),
                directory: slash_path(skill.strip_prefix(root).ok()?)?,
            });
        }
        return None;
    }
    None
}

fn git(home: &Home, checkout: &Path, args: &[&str]) -> Option<String> {
    let repo = home.resolve(checkout).ok()??;
    let control = home.resolve(&checkout.join(".git")).ok()??;
    if repo.through_link || control.through_link {
        return None;
    }
    read_only_git_at(&home.canonical.join(checkout), args)
}

const ORIGIN_URL: &[&str] = &[
    "config",
    "--local",
    "--no-includes",
    "--get",
    "remote.origin.url",
];

pub(crate) fn git_repository_name(checkout: &Path) -> Option<String> {
    let slug = github(&read_only_git_at(checkout, ORIGIN_URL)?)?;
    slug.rsplit('/').next().map(str::to_owned)
}

fn read_only_git_at(checkout: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(key);
        }
    }
    let output = command
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-C",
        ])
        .arg(checkout)
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        // Keep the subprocess on pipes so a write-denied preview never has
        // to open /dev/null for writing.
        .stdin(Stdio::piped())
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

fn origin(home: &Home, checkout: &Path) -> Option<String> {
    github(&git(home, checkout, ORIGIN_URL)?)
}

fn checkout_source(home: &Home, skill: &Path) -> Option<SkillUpstream> {
    let checkout = skill.ancestors().find(|ancestor| {
        home.resolve(&ancestor.join(".git"))
            .ok()
            .flatten()
            .is_some()
    })?;
    let repository = origin(home, checkout)?;
    let path = skill.strip_prefix(checkout).ok()?;
    let skill_md = slash_path(&path.join("SKILL.md"))?;
    let revision = git(home, checkout, &["rev-parse", "--verify", "HEAD"])?;
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let at_revision = format!("{revision}:{skill_md}");
    if git(home, checkout, &["cat-file", "-t", &at_revision])? != "blob" {
        return None;
    }
    Some(SkillUpstream::GitHub {
        repository,
        directory: slash_path(path)?,
        revision: Some(revision),
        checkout: Some(checkout.to_owned()),
    })
}

pub(crate) fn checkout_update(home: &Path, source: &SkillUpstream) -> Option<String> {
    let SkillUpstream::GitHub {
        repository,
        directory,
        revision: Some(revision),
        checkout: Some(checkout),
    } = source
    else {
        return None;
    };
    if !checkout.as_os_str().is_empty() && !super::normal_relative(checkout) {
        return None;
    }
    let home = Home::open(home).ok()?;
    if origin(&home, checkout)? != *repository {
        return None;
    }
    if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let upstream = git(
        &home,
        checkout,
        &["rev-parse", "--symbolic-full-name", "@{upstream}"],
    )
    .filter(|name| name.starts_with("refs/remotes/origin/"))
    .or_else(|| {
        git(
            &home,
            checkout,
            &["symbolic-ref", "refs/remotes/origin/HEAD"],
        )
    })?;
    if !upstream.starts_with("refs/remotes/origin/") {
        return None;
    }
    let head = git(&home, checkout, &["rev-parse", "--verify", &upstream])?;
    if head == *revision {
        return None;
    }
    git(
        &home,
        checkout,
        &["merge-base", "--is-ancestor", revision, &head],
    )?;
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--name-only",
        revision,
        &head,
        "--",
    ];
    if !directory.is_empty() {
        let path = Path::new(directory);
        if !super::normal_relative(path) || slash_path(path).as_deref() != Some(directory) {
            return None;
        }
        args.push(directory);
    }
    let changed = git(&home, checkout, &args)?;
    (!changed.is_empty()).then_some(head)
}
