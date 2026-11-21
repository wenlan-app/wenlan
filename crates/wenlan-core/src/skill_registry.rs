// SPDX-License-Identifier: Apache-2.0
//! Find, check and install other people's agent skills (SKILL.md folders).
//!
//! Search goes to skills.sh's open search address first, the way other
//! desktop tools do, and falls back to wenlan.app's go-between, which calls
//! the documented skills.sh API (that API only accepts a token that lives
//! inside a Vercel deployment). Safety results come from skills.sh's audit
//! endpoint, which needs no token. Files come straight from GitHub at one
//! pinned commit, so what was checked is what lands.
//!
//! A skill is kept in two layers under the Wenlan folder (`~/.wenlan/skills/
//! <name>/`, or `~/.wenlan/spaces/<space>/skills/<name>/` for one Space):
//! `.wenlan/author/` holds the author's files untouched, `.wenlan/my-additions.md`
//! the person's own additions. Each AI tool reads one `SKILL.md`, so the ready
//! copy is the author's files with the additions appended. An author update
//! replaces `author/` and keeps the additions.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, ensure, Context};
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, OpenOptions};
use futures::future::BoxFuture;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use wenlan_types::rulesync::{AdoptedSkillUpdate, SkillUpdateStatus, SkillUpstream};

// The shapes every channel reads live in `wenlan-types`.
pub use wenlan_types::skill_registry::{
    AuditLine, InstalledSkill, LocalSkill, SafetyReport, Scope, SearchVia, SkillHit, UpdateOutcome,
    Verdict,
};

/// Marker file in each tool-folder copy Wenlan wrote. A same-named folder
/// without it belongs to the person and is never touched.
pub const MANAGED_MARKER: &str = ".wenlan-managed";
pub const META_DIR: &str = ".wenlan";
pub const ADDITIONS_FILE: &str = "my-additions.md";
pub const RECORD_FILE: &str = "skill.json";
pub const AUTHOR_DIR: &str = "author";
const ADDITIONS_HEADING: &str = "## My additions";
/// `source` of a skill taken in from an AI tool's folder on this computer
/// rather than downloaded. It has no author to check for updates.
pub const LOCAL_SOURCE: &str = "local";

const MAX_FILES: usize = 100;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 5 * 1024 * 1024;
const MAX_NAME_PROBES: usize = 40;
const SEARCH_LIMIT: usize = 30;

#[path = "skill_registry_project_sync.rs"]
pub(crate) mod project_sync;

/// Where the four services live. Tests point these at a fake.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub skills_sh: String,
    pub wenlan_site: String,
    pub github_api: String,
    pub github_raw: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            skills_sh: "https://skills.sh".into(),
            wenlan_site: "https://wenlan.app".into(),
            github_api: "https://api.github.com".into(),
            github_raw: "https://raw.githubusercontent.com".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The one network call this module makes: a GET.
pub trait Fetch: Send + Sync {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<HttpResponse, String>>;
}

/// The real network, with a short timeout and a plain user agent.
pub struct ReqwestFetch {
    client: reqwest::Client,
}

impl ReqwestFetch {
    pub fn new() -> Result<Self, String> {
        reqwest::Client::builder()
            .user_agent("wenlan-skills")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map(|client| Self { client })
            .map_err(|e| e.to_string())
    }
}

impl Fetch for ReqwestFetch {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<HttpResponse, String>> {
        Box::pin(async move {
            let response = self
                .client
                .get(url)
                .header("Accept", "application/json, text/plain, */*")
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let body = response.bytes().await.map_err(|e| e.to_string())?.to_vec();
            Ok(HttpResponse { status, body })
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("skill search is unavailable right now")]
    SearchUnavailable,
    #[error("{0}")]
    Network(String),
    #[error("this skill failed a safety check")]
    FailedSafetyCheck,
    #[error("the skill's files could not be found in its repository")]
    NotFound,
    #[error("more than one folder in the repository is named {0}")]
    Ambiguous(String),
    #[error("{0}")]
    Refused(String),
    #[error("{path}: {message}")]
    Io { path: String, message: String },
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> RegistryError + '_ {
    move |e| RegistryError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

// ── Search ──────────────────────────────────────────────────────────────

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) fn valid_source(source: &str) -> bool {
    let mut parts = source.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    ok(owner) && ok(repo)
}

pub(crate) fn valid_npm_name(name: &str) -> bool {
    fn component(value: &str) -> bool {
        !value.is_empty()
            && !value.starts_with(['.', '_'])
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
    }
    name.len() <= 214
        && if let Some(scoped) = name.strip_prefix('@') {
            scoped
                .split_once('/')
                .is_some_and(|(scope, name)| component(scope) && component(name))
        } else {
            component(name)
        }
}

pub(crate) fn valid_npm_version(version: &str) -> bool {
    semver::Version::parse(version).is_ok()
}

fn valid_skill_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && !id.contains(['/', '\\', '\0']) && id != "." && id != ".."
}

fn parse_hits(body: &[u8]) -> Option<Vec<SkillHit>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let list = value.get("skills")?.as_array()?;
    let mut hits = Vec::new();
    for item in list {
        let text = |key: &str| item.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let Some(source) = text("source") else {
            continue;
        };
        // Direct search names it skillId; the documented API names it slug.
        let Some(skill_id) = text("skillId").or_else(|| text("slug")) else {
            continue;
        };
        if !valid_source(&source) || !valid_skill_id(&skill_id) {
            continue;
        }
        let name = text("name").unwrap_or_else(|| skill_id.clone());
        let installs = item.get("installs").and_then(|v| v.as_u64()).unwrap_or(0);
        hits.push(SkillHit {
            source,
            skill_id,
            name,
            installs,
        });
    }
    Some(hits)
}

/// Search skills.sh directly; when that fails in any way, ask wenlan.app.
pub async fn search(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    query: &str,
) -> Result<(Vec<SkillHit>, SearchVia), RegistryError> {
    let query = query.trim();
    if query.chars().count() < 2 {
        return Ok((Vec::new(), SearchVia::SkillsSh));
    }
    let q = encode(query);
    let direct = format!(
        "{}/api/search?q={q}&limit={SEARCH_LIMIT}",
        endpoints.skills_sh
    );
    if let Ok(response) = http.get(&direct).await {
        if response.status == 200 {
            if let Some(hits) = parse_hits(&response.body) {
                return Ok((hits, SearchVia::SkillsSh));
            }
        }
    }
    let fallback = format!(
        "{}/api/skills/search?q={q}&limit={SEARCH_LIMIT}",
        endpoints.wenlan_site
    );
    match http.get(&fallback).await {
        Ok(response) if response.status == 200 => parse_hits(&response.body)
            .map(|hits| (hits, SearchVia::WenlanApp))
            .ok_or(RegistryError::SearchUnavailable),
        _ => Err(RegistryError::SearchUnavailable),
    }
}

// ── Safety checks ───────────────────────────────────────────────────────

fn verdict_for(audits: &[AuditLine]) -> Verdict {
    if audits.is_empty() {
        return Verdict::Warn;
    }
    let mut verdict = Verdict::Pass;
    for audit in audits {
        let line = match audit.status.to_ascii_lowercase().as_str() {
            "pass" | "passed" | "safe" => Verdict::Pass,
            "fail" | "failed" | "block" | "blocked" | "malicious" => Verdict::Fail,
            _ => Verdict::Warn,
        };
        verdict = verdict.max(line);
    }
    verdict
}

/// The partner audits skills.sh holds for one skill. A skill with no audit is
/// treated as "warn": it is never shown as checked.
pub async fn safety(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    source: &str,
    skill_id: &str,
) -> Result<SafetyReport, RegistryError> {
    if !valid_source(source) || !valid_skill_id(skill_id) {
        return Err(RegistryError::Refused("not a skills.sh skill".into()));
    }
    let url = format!(
        "{}/api/v1/skills/audit/{source}/{}",
        endpoints.skills_sh,
        encode(skill_id)
    );
    let response = http.get(&url).await.map_err(RegistryError::Network)?;
    if response.status == 404 {
        return Ok(SafetyReport {
            verdict: Verdict::Warn,
            audits: Vec::new(),
        });
    }
    if response.status != 200 {
        return Err(RegistryError::Network(format!(
            "skills.sh answered {}",
            response.status
        )));
    }
    let value: serde_json::Value = serde_json::from_slice(&response.body)
        .map_err(|_| RegistryError::Network("unreadable safety results".into()))?;
    let audits: Vec<AuditLine> = value
        .get("audits")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .map(|a| {
                    let text = |k: &str| {
                        a.get(k)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string()
                    };
                    AuditLine {
                        provider: text("provider"),
                        status: text("status"),
                        summary: text("summary"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SafetyReport {
        verdict: verdict_for(&audits),
        audits,
    })
}

// ── Files from GitHub ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    /// Relative to the skill folder, `/`-separated.
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct FetchedSkill {
    pub source: String,
    pub skill_id: String,
    pub commit: String,
    /// The skill folder inside the repository ("" for the repo root).
    pub repo_path: String,
    pub files: Vec<SkillFile>,
    pub hash: String,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    mode: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Tree {
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

fn safe_relative(path: &str) -> bool {
    if path.is_empty() || path.contains(['\\', '\0']) || path.starts_with('/') {
        return false;
    }
    Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
}

fn raw_url(endpoints: &Endpoints, source: &str, commit: &str, path: &str) -> String {
    let encoded: Vec<String> = path.split('/').map(encode).collect();
    format!(
        "{}/{source}/{commit}/{}",
        endpoints.github_raw,
        encoded.join("/")
    )
}

/// A file's text without the byte-order mark some editors put at its very
/// start. Only what Wenlan reads from the text skips the mark; the bytes it
/// keeps and delivers are the author's.
pub fn without_leading_bom(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// One field (`name:`, `description:`) of a SKILL.md's front matter.
pub fn front_matter_field(skill_md: &str, field: &str) -> Option<String> {
    let rest = without_leading_bom(skill_md).strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&rest[..end]).ok()?;
    yaml.get(field)?.as_str().map(|s| s.trim().to_string())
}

fn dir_of(skill_md_path: &str) -> &str {
    skill_md_path
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("")
}

fn hash_files(files: &[SkillFile]) -> String {
    let mut hasher = Sha256::new();
    let mut sorted: Vec<&SkillFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    for file in sorted {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update((file.bytes.len() as u64).to_le_bytes());
        hasher.update(&file.bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn get_ok(http: &dyn Fetch, url: &str) -> Result<Vec<u8>, RegistryError> {
    let response = http.get(url).await.map_err(RegistryError::Network)?;
    match response.status {
        200 => Ok(response.body),
        404 => Err(RegistryError::NotFound),
        403 | 429 => Err(RegistryError::Network(
            "GitHub is limiting requests; try again in a few minutes".into(),
        )),
        status => Err(RegistryError::Network(format!("GitHub answered {status}"))),
    }
}

/// The newest commit on the repository's default branch.
pub async fn head_commit(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    source: &str,
) -> Result<String, RegistryError> {
    if !valid_source(source) {
        return Err(RegistryError::Refused("not a GitHub repository".into()));
    }
    let body = get_ok(
        http,
        &format!("{}/repos/{source}/commits/HEAD", endpoints.github_api),
    )
    .await?;
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| RegistryError::Network("unreadable commit".into()))?;
    let sha = value
        .get("sha")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(sha.to_ascii_lowercase())
    } else {
        Err(RegistryError::Network("unreadable commit".into()))
    }
}

/// Every file of one skill at one commit. The folder is found the way the
/// skills CLI lists it: `repo_path` when known, else a folder named after the
/// id, else the one SKILL.md whose `name:` is the id.
pub async fn fetch_skill(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    source: &str,
    skill_id: &str,
    commit: &str,
    repo_path: Option<&str>,
) -> Result<FetchedSkill, RegistryError> {
    if !valid_source(source) || !valid_skill_id(skill_id) {
        return Err(RegistryError::Refused("not a skills.sh skill".into()));
    }
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(RegistryError::Refused("not a pinned commit".into()));
    }
    let body = get_ok(
        http,
        &format!(
            "{}/repos/{source}/git/trees/{commit}?recursive=1",
            endpoints.github_api
        ),
    )
    .await?;
    let tree: Tree = serde_json::from_slice(&body)
        .map_err(|_| RegistryError::Network("unreadable repository listing".into()))?;
    if tree.truncated {
        return Err(RegistryError::Refused(
            "the repository is too large to read".into(),
        ));
    }
    let skill_mds: Vec<&str> = tree
        .tree
        .iter()
        .filter(|e| e.kind == "blob" && (e.path == "SKILL.md" || e.path.ends_with("/SKILL.md")))
        .map(|e| e.path.as_str())
        .collect();

    let chosen: String = 'found: {
        if let Some(path) = repo_path {
            let wanted = if path.is_empty() {
                "SKILL.md".to_string()
            } else {
                format!("{path}/SKILL.md")
            };
            if skill_mds.contains(&wanted.as_str()) {
                break 'found path.to_string();
            }
        }
        let same_named: Vec<&str> = skill_mds
            .iter()
            .copied()
            .filter(|p| dir_of(p).rsplit('/').next() == Some(skill_id))
            .collect();
        match same_named.len() {
            1 => break 'found dir_of(same_named[0]).to_string(),
            n if n > 1 => return Err(RegistryError::Ambiguous(skill_id.into())),
            _ => {}
        }
        let mut matches = Vec::new();
        for path in skill_mds.iter().take(MAX_NAME_PROBES) {
            let text = get_ok(http, &raw_url(endpoints, source, commit, path)).await?;
            let text = String::from_utf8_lossy(&text);
            if front_matter_field(&text, "name").as_deref() == Some(skill_id) {
                matches.push(dir_of(path).to_string());
            }
        }
        match matches.len() {
            1 => matches.remove(0),
            0 => return Err(RegistryError::NotFound),
            _ => return Err(RegistryError::Ambiguous(skill_id.into())),
        }
    };

    let prefix = if chosen.is_empty() {
        String::new()
    } else {
        format!("{chosen}/")
    };
    let mut wanted = Vec::new();
    let mut total = 0u64;
    for entry in &tree.tree {
        if !entry.path.starts_with(&prefix) || entry.kind == "tree" {
            continue;
        }
        let relative = &entry.path[prefix.len()..];
        if entry.kind != "blob" || entry.mode == "120000" {
            return Err(RegistryError::Refused(format!(
                "{relative} is a link or submodule, which Wenlan does not install"
            )));
        }
        if !safe_relative(relative) {
            return Err(RegistryError::Refused(format!("unsafe path {relative}")));
        }
        let size = entry.size.unwrap_or(0);
        if size > MAX_FILE_BYTES {
            return Err(RegistryError::Refused(format!("{relative} is over 1 MB")));
        }
        total += size;
        wanted.push((entry.path.clone(), relative.to_string()));
    }
    if wanted.len() > MAX_FILES || total > MAX_TOTAL_BYTES {
        return Err(RegistryError::Refused(
            "the skill is larger than Wenlan installs (100 files, 5 MB)".into(),
        ));
    }
    let mut files = Vec::with_capacity(wanted.len());
    for (full, relative) in wanted {
        let bytes = get_ok(http, &raw_url(endpoints, source, commit, &full)).await?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(RegistryError::Refused(format!("{relative} is over 1 MB")));
        }
        files.push(SkillFile {
            path: relative,
            bytes,
        });
    }
    if !files.iter().any(|f| f.path == "SKILL.md") {
        return Err(RegistryError::NotFound);
    }
    let hash = hash_files(&files);
    Ok(FetchedSkill {
        source: source.into(),
        skill_id: skill_id.into(),
        commit: commit.to_ascii_lowercase(),
        repo_path: chosen,
        files,
        hash,
    })
}

const SCRIPT_EXTENSIONS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "py", "js", "mjs", "cjs", "ts", "rb", "pl", "php", "ps1", "psm1",
    "bat", "cmd", "go", "rs", "lua", "exe", "dll", "so", "dylib", "jar", "wasm",
];

/// Files that can run on the person's computer, which Wenlan never runs.
pub fn runnable_files(files: &[SkillFile]) -> Vec<String> {
    files
        .iter()
        .filter(|f| {
            let ext = f
                .path
                .rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .unwrap_or_default();
            SCRIPT_EXTENSIONS.contains(&ext.as_str())
                || f.bytes.starts_with(b"#!")
                || f.bytes.starts_with(b"\x7fELF")
                || f.bytes.starts_with(b"MZ")
        })
        .map(|f| f.path.clone())
        .collect()
}

// ── Storage under the Wenlan folder ─────────────────────────────────────
//
// All Spaces: `<wenlan>/skills/<name>/`. One Space:
// `<wenlan>/spaces/<space folder>/skills/<name>/`. Each downloaded skill
// folder holds the ready copy AI tools get (SKILL.md with the additions at
// the end, plus the author's other files) and a `.wenlan/` folder:
// `author/` (the author's files, untouched), `skill.json` (the record) and
// `my-additions.md`. A folder without `.wenlan/skill.json` is not one of
// these and is never touched.

/// `~/.wenlan`, the folder that holds `pages` (`<data root>` for an isolated
/// run, so tests never reach the real one).
pub fn default_wenlan_root() -> PathBuf {
    let pages = crate::config::default_pages_root();
    pages
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The skills folder for one scope.
pub fn skills_dir(root: &Path, scope: &Scope) -> PathBuf {
    match scope {
        Scope::All => root.join("skills"),
        Scope::Space { name } => root
            .join("spaces")
            .join(crate::space_folders::folder_name(name))
            .join("skills"),
    }
}

fn skill_dir(root: &Path, scope: &Scope, name: &str) -> PathBuf {
    skills_dir(root, scope).join(name)
}

fn meta(dir: &Path) -> PathBuf {
    dir.join(META_DIR)
}

/// A folder name every tool accepts: lower-case letters, digits and dashes.
pub fn folder_name(skill_md: &str, skill_id: &str) -> Option<String> {
    let raw = front_matter_field(skill_md, "name").unwrap_or_else(|| skill_id.to_string());
    let mut out = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out
        .trim_end_matches('-')
        .chars()
        .take(64)
        .collect::<String>();
    let out = out.trim_end_matches('-').to_string();
    (!out.is_empty()).then_some(out)
}

fn write_all(dir: &Path, files: &[SkillFile]) -> Result<(), RegistryError> {
    for file in files {
        if !safe_relative(&file.path) {
            return Err(RegistryError::Refused(format!("unsafe path {}", file.path)));
        }
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        std::fs::write(&path, &file.bytes).map_err(io_err(&path))?;
    }
    Ok(())
}

fn sibling(target: &Path, kind: &str) -> PathBuf {
    target.with_file_name(format!(
        ".{}.wenlan-{kind}",
        target.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// Swap `staged` into `target`, keeping the old copy until the new one is in
/// place.
fn swap_in(staged: &Path, target: &Path) -> Result<(), RegistryError> {
    let aside = sibling(target, "old");
    let _ = std::fs::remove_dir_all(&aside);
    let had_old = target.exists();
    if had_old {
        std::fs::rename(target, &aside).map_err(io_err(target))?;
    }
    if let Err(error) = std::fs::rename(staged, target) {
        if had_old {
            let _ = std::fs::rename(&aside, target);
        }
        let _ = std::fs::remove_dir_all(staged);
        return Err(io_err(target)(error));
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&aside);
    }
    Ok(())
}

fn read_record_at(dir: &Path) -> Option<InstalledSkill> {
    let text = std::fs::read_to_string(meta(dir).join(RECORD_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn records_in(skills: &Path, out: &mut Vec<(PathBuf, InstalledSkill)>) {
    let Ok(entries) = std::fs::read_dir(skills) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let dir = entry.path();
        if let Some(record) = read_record_at(&dir) {
            out.push((dir, record));
        }
    }
}

fn all_records(root: &Path) -> Vec<(PathBuf, InstalledSkill)> {
    let mut out = Vec::new();
    records_in(&root.join("skills"), &mut out);
    if let Ok(spaces) = std::fs::read_dir(root.join("spaces")) {
        for space in spaces.flatten() {
            records_in(&space.path().join("skills"), &mut out);
        }
    }
    out
}

fn find(root: &Path, name: &str) -> Option<(PathBuf, InstalledSkill)> {
    all_records(root).into_iter().find(|(_, r)| r.name == name)
}

pub fn read_record(root: &Path, name: &str) -> Option<InstalledSkill> {
    find(root, name).map(|(_, record)| record)
}

/// Every downloaded skill, by name.
pub fn list_installed(root: &Path) -> Vec<InstalledSkill> {
    let mut out: Vec<InstalledSkill> = all_records(root).into_iter().map(|(_, r)| r).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn read_additions(root: &Path, name: &str) -> String {
    find(root, name)
        .and_then(|(dir, _)| std::fs::read_to_string(meta(&dir).join(ADDITIONS_FILE)).ok())
        .unwrap_or_default()
}

/// The SKILL.md an AI tool reads: the author's, a Space line when the skill is
/// for one Space, and the person's additions at the end.
pub fn compose_skill_md(author: &str, scope: &Scope, additions: &str) -> String {
    let mut out = String::new();
    // A byte-order mark before the front matter stays where it is.
    let mark = author.len() - without_leading_bom(author).len();
    let (front, body) = match author[mark..].strip_prefix("---").and_then(|rest| {
        rest.find("\n---").map(|end| {
            let close = mark + 3 + end + 4;
            let close = author[close..]
                .find('\n')
                .map(|n| close + n + 1)
                .unwrap_or(author.len());
            (&author[..close], &author[close..])
        })
    }) {
        Some(split) => split,
        None => ("", author),
    };
    out.push_str(front);
    if let Scope::Space { name } = scope {
        out.push_str(&format!(
            "\n> Use this skill only when working in the Wenlan Space \"{name}\".\n"
        ));
    }
    out.push_str(body);
    let additions = additions.trim();
    if !additions.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("\n{ADDITIONS_HEADING}\n\n{additions}\n"));
    }
    out
}

fn read_tree(
    dir: &Path,
    base: &Path,
    skip_meta: bool,
    out: &mut Vec<SkillFile>,
) -> Result<(), RegistryError> {
    for entry in std::fs::read_dir(dir).map_err(io_err(dir))?.flatten() {
        let path = entry.path();
        if skip_meta && dir == base && entry.file_name() == META_DIR {
            continue;
        }
        let kind = entry.file_type().map_err(io_err(&path))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            read_tree(&path, base, skip_meta, out)?;
        } else {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push(SkillFile {
                path: relative,
                bytes: std::fs::read(&path).map_err(io_err(&path))?,
            });
        }
    }
    Ok(())
}

/// The ready copy: the author's files with SKILL.md composed.
fn ready_files(dir: &Path, record: &InstalledSkill) -> Result<Vec<SkillFile>, RegistryError> {
    let author = meta(dir).join(AUTHOR_DIR);
    let mut files = Vec::new();
    read_tree(&author, &author, false, &mut files)?;
    let additions = std::fs::read_to_string(meta(dir).join(ADDITIONS_FILE)).unwrap_or_default();
    compose_ready_files(&mut files, &record.scope, &additions);
    Ok(files)
}

fn compose_ready_files(files: &mut [SkillFile], scope: &Scope, additions: &str) {
    for file in files.iter_mut().filter(|file| file.path == "SKILL.md") {
        let text = String::from_utf8_lossy(&file.bytes);
        file.bytes = compose_skill_md(&text, scope, additions).into_bytes();
    }
}

/// Snapshot the held skills that belong in the personal tool setup. A read
/// error aborts the snapshot so the applier cannot mistake it for a removal.
pub fn personal_sources(
    root: &Path,
) -> anyhow::Result<BTreeMap<String, BTreeMap<PathBuf, Vec<u8>>>> {
    let Some(root) = optional_dir(Dir::open_ambient_dir(root, cap_std::ambient_authority()))?
    else {
        return Ok(BTreeMap::new());
    };
    let Some(skills) = optional_dir(root.open_dir_nofollow("skills"))? else {
        return Ok(BTreeMap::new());
    };
    let mut sources = BTreeMap::new();
    for entry in skills.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| !name.starts_with('.')) else {
            continue;
        };
        let kind = entry.file_type()?;
        ensure!(!kind.is_symlink(), "the stored skill {name} is a symlink");
        if !kind.is_dir() {
            continue;
        }
        let skill = skills.open_dir_nofollow(name)?;
        let Some(control) = optional_dir(skill.open_dir_nofollow(META_DIR))? else {
            continue;
        };
        let record: InstalledSkill = serde_json::from_reader(open_regular(&control, RECORD_FILE)?)
            .with_context(|| format!("read registry record for {name}"))?;
        if record.scope != Scope::All {
            continue;
        }
        ensure!(
            record.name == name && local_name(Path::new(name)).as_deref() == Some(name),
            "invalid registry name for {name}"
        );
        ensure!(
            record.verdict != Verdict::Fail,
            "{name} failed its skill checks"
        );
        let author = control.open_dir_nofollow(AUTHOR_DIR)?;
        let mut files = Vec::new();
        let mut total = 0;
        read_personal_author(&author, "", &mut files, &mut total)
            .with_context(|| format!("read stored author files for {name}"))?;
        ensure!(
            files.iter().any(|file| file.path == "SKILL.md") && hash_files(&files) == record.hash,
            "the stored author files for {name} differ from the version Wenlan took in"
        );
        if let Some(reason) = hidden_from_the_person(&files) {
            bail!("{name} contains text the preview cannot show in full. {reason}");
        }
        let mut additions = String::new();
        open_regular(&control, ADDITIONS_FILE)?.read_to_string(&mut additions)?;
        compose_ready_files(&mut files, &record.scope, &additions);
        sources.insert(
            record.name,
            files
                .into_iter()
                .map(|file| (PathBuf::from(file.path), file.bytes))
                .collect(),
        );
    }
    Ok(sources)
}

fn optional_dir(dir: std::io::Result<Dir>) -> std::io::Result<Option<Dir>> {
    match dir {
        Ok(dir) => Ok(Some(dir)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn open_regular(dir: &Dir, name: &str) -> anyhow::Result<cap_std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = dir
        .open_with(name, &options)
        .with_context(|| format!("open stored skill file {name}"))?;
    ensure!(file.metadata()?.is_file(), "{name} is not a regular file");
    Ok(file)
}

fn read_personal_author(
    dir: &Dir,
    prefix: &str,
    files: &mut Vec<SkillFile>,
    total: &mut u64,
) -> anyhow::Result<()> {
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("skill file name is not UTF-8")?;
        let path = format!("{prefix}{name}");
        ensure!(safe_relative(&path), "invalid stored skill path {path}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            read_personal_author(
                &dir.open_dir_nofollow(name)?,
                &format!("{path}/"),
                files,
                total,
            )?;
            continue;
        }
        ensure!(kind.is_file(), "{path} is not a regular file");
        ensure!(files.len() < MAX_FILES, "stored skill has too many files");
        let limit = MAX_FILE_BYTES.min(MAX_TOTAL_BYTES - *total);
        let mut bytes = Vec::new();
        open_regular(dir, name)?
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= limit,
            "stored skill exceeds its size limit"
        );
        *total += bytes.len() as u64;
        files.push(SkillFile { path, bytes });
    }
    Ok(())
}

/// Write `dir` again from its `.wenlan/` folder: a fresh ready copy beside
/// the same `.wenlan/`, swapped in whole.
fn rebuild(dir: &Path, record: &InstalledSkill) -> Result<(), RegistryError> {
    let files = ready_files(dir, record)?;
    let staged = sibling(dir, "new");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(io_err(&staged))?;
    if let Err(error) = write_all(&staged, &files) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(meta(dir), meta(&staged)) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(io_err(dir)(error));
    }
    let aside = sibling(dir, "old");
    let _ = std::fs::remove_dir_all(&aside);
    let undo = |error: std::io::Error, moved_dir: bool| {
        if moved_dir {
            let _ = std::fs::rename(&aside, dir);
        }
        let _ = std::fs::rename(meta(&staged), meta(dir));
        let _ = std::fs::remove_dir_all(&staged);
        io_err(dir)(error)
    };
    if let Err(error) = std::fs::rename(dir, &aside) {
        return Err(undo(error, false));
    }
    if let Err(error) = std::fs::rename(&staged, dir) {
        return Err(undo(error, true));
    }
    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

fn write_record(dir: &Path, record: &InstalledSkill) -> Result<(), RegistryError> {
    let path = meta(dir).join(RECORD_FILE);
    let text = serde_json::to_string_pretty(record).map_err(|e| RegistryError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io_err(&tmp))?;
    std::fs::rename(&tmp, &path).map_err(io_err(&path))
}

/// Write the author's files into the Wenlan folder under `scope`, keep any
/// additions, and rebuild the ready copy. Refuses a name that holds a
/// different skill, or a folder Wenlan did not download.
pub fn store_skill(
    root: &Path,
    fetched: &FetchedSkill,
    verdict: Verdict,
    scope: Scope,
    now: i64,
) -> Result<InstalledSkill, RegistryError> {
    if verdict == Verdict::Fail {
        return Err(RegistryError::FailedSafetyCheck);
    }
    let skill_md = fetched
        .files
        .iter()
        .find(|f| f.path == "SKILL.md")
        .map(|f| String::from_utf8_lossy(&f.bytes).to_string())
        .ok_or(RegistryError::NotFound)?;
    let name = folder_name(&skill_md, &fetched.skill_id)
        .ok_or_else(|| RegistryError::Refused("the skill has no usable name".into()))?;
    store_as(root, name, &skill_md, fetched, verdict, scope, now)
}

fn store_as(
    root: &Path,
    name: String,
    skill_md: &str,
    fetched: &FetchedSkill,
    verdict: Verdict,
    scope: Scope,
    now: i64,
) -> Result<InstalledSkill, RegistryError> {
    let previous = find(root, &name);
    if let Some((_, prev)) = &previous {
        if prev.source != fetched.source || prev.skill_id != fetched.skill_id {
            return Err(RegistryError::Refused(format!(
                "a skill named {name} is already installed from {}",
                prev.source
            )));
        }
    }
    let dir = skill_dir(root, &scope, &name);
    match &previous {
        Some((old_dir, _)) if *old_dir != dir => {
            if dir.exists() {
                return Err(RegistryError::Refused(format!(
                    "{} already exists",
                    dir.display()
                )));
            }
            std::fs::create_dir_all(skills_dir(root, &scope)).map_err(io_err(&dir))?;
            std::fs::rename(old_dir, &dir).map_err(io_err(old_dir))?;
        }
        Some(_) => {}
        None if dir.exists() => {
            return Err(RegistryError::Refused(format!(
                "{} already exists and was not installed by Wenlan",
                dir.display()
            )));
        }
        None => {}
    }
    let meta_dir = meta(&dir);
    std::fs::create_dir_all(&meta_dir).map_err(io_err(&meta_dir))?;
    let author = meta_dir.join(AUTHOR_DIR);
    let staged = sibling(&author, "new");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(io_err(&staged))?;
    if let Err(error) = write_all(&staged, &fetched.files) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }
    swap_in(&staged, &author)?;
    let additions = meta_dir.join(ADDITIONS_FILE);
    if !additions.exists() {
        std::fs::write(&additions, "").map_err(io_err(&additions))?;
    }
    let record = InstalledSkill {
        name,
        source: fetched.source.clone(),
        skill_id: fetched.skill_id.clone(),
        repo_path: fetched.repo_path.clone(),
        commit: fetched.commit.clone(),
        hash: fetched.hash.clone(),
        verdict,
        scope,
        description: front_matter_field(skill_md, "description"),
        installed_at: previous
            .as_ref()
            .map(|(_, p)| p.installed_at)
            .unwrap_or(now),
        updated_at: now,
    };
    write_record(&dir, &record)?;
    rebuild(&dir, &record)?;
    Ok(record)
}

pub fn set_additions(root: &Path, name: &str, text: &str) -> Result<(), RegistryError> {
    let (dir, record) = find(root, name).ok_or(RegistryError::NotFound)?;
    let path = meta(&dir).join(ADDITIONS_FILE);
    std::fs::write(&path, text).map_err(io_err(&path))?;
    rebuild(&dir, &record)
}

/// Move a skill to another scope (all Spaces, or one Space).
pub fn set_scope(root: &Path, name: &str, scope: Scope) -> Result<InstalledSkill, RegistryError> {
    let (dir, mut record) = find(root, name).ok_or(RegistryError::NotFound)?;
    let target = skill_dir(root, &scope, name);
    if target != dir {
        if target.exists() {
            return Err(RegistryError::Refused(format!(
                "{} already exists",
                target.display()
            )));
        }
        std::fs::create_dir_all(skills_dir(root, &scope)).map_err(io_err(&target))?;
        std::fs::rename(&dir, &target).map_err(io_err(&dir))?;
    }
    record.scope = scope;
    write_record(&target, &record)?;
    rebuild(&target, &record)?;
    Ok(record)
}

/// Copy the ready copy into each tool skills folder. A same-named folder
/// without Wenlan's marker belongs to the person: it is skipped and named in
/// the result. Returns (written folders, skipped folders).
pub fn deliver(
    root: &Path,
    name: &str,
    tool_dirs: &[PathBuf],
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), RegistryError> {
    let (dir, record) = find(root, name).ok_or(RegistryError::NotFound)?;
    if crate::rulesync_personal::enabled() {
        return Ok((Vec::new(), tool_dirs.to_vec()));
    }
    let mut files = Vec::new();
    read_tree(&dir, &dir, true, &mut files)?;
    files.push(SkillFile {
        path: MANAGED_MARKER.into(),
        bytes: format!("{}@{}\n", record.source, record.commit).into_bytes(),
    });
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    for tool in tool_dirs {
        let target = tool.join(name);
        if target.exists() && !target.join(MANAGED_MARKER).is_file() {
            skipped.push(target);
            continue;
        }
        std::fs::create_dir_all(tool).map_err(io_err(tool))?;
        let staged = sibling(&target, "new");
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(&staged).map_err(io_err(&staged))?;
        if let Err(error) = write_all(&staged, &files) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(error);
        }
        swap_in(&staged, &target)?;
        written.push(target);
    }
    Ok((written, skipped))
}

/// Remove the tool copies Wenlan wrote and the skill from the Wenlan folder.
pub fn remove(root: &Path, name: &str, tool_dirs: &[PathBuf]) -> Result<(), RegistryError> {
    let (dir, _) = find(root, name).ok_or(RegistryError::NotFound)?;
    if !crate::rulesync_personal::enabled() {
        for tool in tool_dirs {
            let target = tool.join(name);
            if target.join(MANAGED_MARKER).is_file() {
                std::fs::remove_dir_all(&target).map_err(io_err(&target))?;
            }
        }
    }
    std::fs::remove_dir_all(&dir).map_err(io_err(&dir))
}

/// Check one installed skill against its repository. A new version that
/// passes every check replaces the author's files (additions kept); one that
/// does not is left for the person to look at.
pub async fn refresh_one(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    root: &Path,
    name: &str,
    now: i64,
) -> Result<UpdateOutcome, RegistryError> {
    let (dir, record) = find(root, name).ok_or(RegistryError::NotFound)?;
    let outcome = |o: &str| UpdateOutcome {
        name: name.into(),
        outcome: o.into(),
    };
    if record.source == LOCAL_SOURCE {
        return Ok(outcome("unchanged"));
    }
    let Some(fetched) = latest_github_skill(
        http,
        endpoints,
        &record.source,
        &record.skill_id,
        &record.repo_path,
        Some(&record.commit),
    )
    .await?
    else {
        return Ok(outcome("unchanged"));
    };
    if fetched.hash == record.hash {
        let mut moved = record.clone();
        moved.commit.clone_from(&fetched.commit);
        write_record(&dir, &moved)?;
        return Ok(outcome("unchanged"));
    }
    let report = safety(http, endpoints, &record.source, &record.skill_id).await?;
    if report.verdict != Verdict::Pass || hidden_from_the_person(&fetched.files).is_some() {
        return Ok(outcome("needs_look"));
    }
    store_skill(root, &fetched, report.verdict, record.scope.clone(), now)?;
    Ok(outcome("updated"))
}

async fn latest_github_skill(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    source: &str,
    skill_id: &str,
    directory: &str,
    revision: Option<&str>,
) -> Result<Option<FetchedSkill>, RegistryError> {
    let head = head_commit(http, endpoints, source).await?;
    if revision == Some(head.as_str()) {
        return Ok(None);
    }
    fetch_skill(http, endpoints, source, skill_id, &head, Some(directory))
        .await
        .map(Some)
}

/// Offer releases for adopted skills through the registry's update check.
/// The master and its tool files keep their approved bytes until the person
/// decides what to do with the offer.
pub async fn adopted_updates(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    home: &Path,
    skills: &[crate::rulesync_adopt::AdoptedSkill],
) -> Vec<AdoptedSkillUpdate> {
    let mut updates = Vec::new();
    for skill in skills {
        let Some(upstream) = &skill.upstream else {
            continue;
        };
        let result = match upstream {
            SkillUpstream::GitHub {
                repository,
                directory,
                revision,
                ..
            } => {
                match latest_github_skill(
                    http,
                    endpoints,
                    repository,
                    &skill.name,
                    directory,
                    revision.as_deref(),
                )
                .await
                {
                    Ok(Some(fetched)) if fetched.repo_path == *directory => {
                        let same = fetched.files.len() == skill.files.len()
                            && fetched.files.iter().all(|file| {
                                skill.files.get(Path::new(&file.path)) == Some(&file.bytes)
                            });
                        Ok((!same).then_some(fetched.commit))
                    }
                    Ok(Some(_)) => Err(RegistryError::NotFound),
                    Ok(None) => Ok(None),
                    Err(error) => crate::rulesync_adopt::checkout_update(home, upstream)
                        .map_or(Err(error), |head| Ok(Some(head))),
                }
            }
            SkillUpstream::Npm {
                package, version, ..
            } => newer_npm_release(http, package, version).await,
        };
        let status = match result {
            Ok(Some(revision)) => SkillUpdateStatus::Available { revision },
            Ok(None) => SkillUpdateStatus::Current,
            Err(error) => SkillUpdateStatus::Unavailable {
                reason: error.to_string(),
            },
        };
        updates.push(AdoptedSkillUpdate {
            name: skill.name.clone(),
            upstream: upstream.clone(),
            status,
        });
    }
    updates
}

async fn newer_npm_release(
    http: &dyn Fetch,
    package: &str,
    installed: &str,
) -> Result<Option<String>, RegistryError> {
    if !valid_npm_name(package) {
        return Err(RegistryError::Refused("invalid npm package name".into()));
    }
    let current = semver::Version::parse(installed)
        .map_err(|_| RegistryError::Refused("invalid installed npm version".into()))?;
    let url = format!("https://registry.npmjs.org/{}/latest", encode(package));
    let response = http.get(&url).await.map_err(RegistryError::Network)?;
    if response.status != 200 {
        return Err(RegistryError::Network(format!(
            "npm registry answered {}",
            response.status
        )));
    }
    let invalid = || RegistryError::Network("unreadable npm release".into());
    let release: serde_json::Value =
        serde_json::from_slice(&response.body).map_err(|_| invalid())?;
    if release["name"].as_str() != Some(package) {
        return Err(invalid());
    }
    let latest = semver::Version::parse(release["version"].as_str().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    Ok(latest
        .cmp_precedence(&current)
        .is_gt()
        .then(|| latest.to_string()))
}

// ── Skills already on this computer ─────────────────────────────────────
//
// Folders the person (or another installer) put in an AI tool's skills
// folder. Importing copies them into the Wenlan folder; the original is
// never changed, so the tool that had it keeps using it, and the copy can
// reach the other tools.

/// A folder name that is already tool-safe, so the Wenlan copy keeps the
/// same name and never lands beside the original as a second skill.
fn local_name(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_str()?.to_string();
    (folder_name("", &name).as_deref() == Some(name.as_str())).then_some(name)
}

/// Skills in the given tool folders that Wenlan does not hold yet. Copies
/// Wenlan wrote (they carry the marker) and folders without SKILL.md are
/// left out; a name found in two tools is listed once.
pub fn scan_local(root: &Path, tool_dirs: &[PathBuf]) -> Vec<LocalSkill> {
    scan_local_where(root, tool_dirs, |_| true)
}

/// The opt-in writer's scan reads only source folders under the selected
/// AI-tool home. This also keeps a scratch scan from following links into the
/// person's usual setup.
pub fn scan_local_in_home(root: &Path, tool_dirs: &[PathBuf], home: &Path) -> Vec<LocalSkill> {
    if !std::fs::symlink_metadata(home).is_ok_and(|meta| meta.is_dir()) {
        return Vec::new();
    }
    let Ok(home) = std::fs::canonicalize(home) else {
        return Vec::new();
    };
    let under_home =
        |dir: &Path| std::fs::canonicalize(dir).is_ok_and(|path| path.starts_with(&home));
    let tool_dirs: Vec<_> = tool_dirs
        .iter()
        .filter(|dir| under_home(dir))
        .cloned()
        .collect();
    scan_local_where(root, &tool_dirs, |dir| {
        under_home(dir)
            && std::fs::symlink_metadata(dir.join("SKILL.md")).is_ok_and(|m| m.is_file())
    })
}

fn scan_local_where(
    root: &Path,
    tool_dirs: &[PathBuf],
    allow: impl Fn(&Path) -> bool,
) -> Vec<LocalSkill> {
    let held: std::collections::HashSet<String> =
        all_records(root).into_iter().map(|(_, r)| r.name).collect();
    let mut out: Vec<LocalSkill> = Vec::new();
    for tool in tool_dirs {
        let Ok(entries) = std::fs::read_dir(tool) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        found.sort();
        for dir in found {
            let Some(name) = local_name(&dir) else {
                continue;
            };
            if held.contains(&name)
                || out.iter().any(|s| s.name == name)
                || !allow(&dir)
                || !dir.is_dir()
                || dir.join(MANAGED_MARKER).exists()
            {
                continue;
            }
            let Ok(skill_md) = std::fs::read_to_string(dir.join("SKILL.md")) else {
                continue;
            };
            out.push(LocalSkill {
                description: front_matter_field(&skill_md, "description"),
                path: dir.display().to_string(),
                name,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Copy one tool-folder skill into the Wenlan folder for every Space. The
/// original folder is only read. No check has run on it, so it is recorded
/// as "warn".
pub fn import_local(root: &Path, dir: &Path, now: i64) -> Result<InstalledSkill, RegistryError> {
    import_local_checked(root, dir, now, |_| None)
}

/// `import_local`, but a folder holding text a preview could not show in
/// full is refused (see `hidden_from_the_person`).
pub fn import_local_fully_shown(
    root: &Path,
    dir: &Path,
    now: i64,
) -> Result<InstalledSkill, RegistryError> {
    import_local_checked(root, dir, now, hidden_from_the_person)
}

fn import_local_checked(
    root: &Path,
    dir: &Path,
    now: i64,
    refuse: impl Fn(&[SkillFile]) -> Option<String>,
) -> Result<InstalledSkill, RegistryError> {
    let name = local_name(dir)
        .ok_or_else(|| RegistryError::Refused(format!("{} has no usable name", dir.display())))?;
    if dir.join(MANAGED_MARKER).exists() {
        return Err(RegistryError::Refused(format!(
            "{} is a copy Wenlan wrote",
            dir.display()
        )));
    }
    if find(root, &name).is_some() {
        return Err(RegistryError::Refused(format!(
            "a skill named {name} is already in Wenlan"
        )));
    }
    let mut files = Vec::new();
    read_tree(dir, dir, false, &mut files)?;
    let total: u64 = files.iter().map(|f| f.bytes.len() as u64).sum();
    if files.len() > MAX_FILES
        || total > MAX_TOTAL_BYTES
        || files.iter().any(|f| f.bytes.len() as u64 > MAX_FILE_BYTES)
    {
        return Err(RegistryError::Refused(format!(
            "{} is too large to take in",
            dir.display()
        )));
    }
    if let Some(reason) = refuse(&files) {
        return Err(RegistryError::Refused(reason));
    }
    let skill_md = files
        .iter()
        .find(|f| f.path == "SKILL.md")
        .map(|f| String::from_utf8_lossy(&f.bytes).to_string())
        .ok_or(RegistryError::NotFound)?;
    let fetched = FetchedSkill {
        source: LOCAL_SOURCE.into(),
        skill_id: name.clone(),
        commit: String::new(),
        repo_path: dir.display().to_string(),
        hash: hash_files(&files),
        files,
    };
    store_as(
        root,
        name,
        &skill_md,
        &fetched,
        Verdict::Warn,
        Scope::All,
        now,
    )
}

/// Text of each file for showing before install; binary files are listed
/// without text.
pub fn preview_text(files: &[SkillFile]) -> BTreeMap<String, Option<String>> {
    files
        .iter()
        .map(|f| {
            let text = std::str::from_utf8(&f.bytes)
                .ok()
                .map(|t| t.chars().take(64 * 1024).collect::<String>());
            (f.path.clone(), text)
        })
        .collect()
}

/// How much of one text file a preview shows.
pub const PREVIEW_CHARS: usize = 64 * 1024;

/// Characters that take no room on the screen or change how the text
/// around them is drawn: zero-width and joining marks, direction marks and
/// overrides, variation selectors, invisible fillers and the tag block. A
/// skill could use them to put words in a file that the person reading it
/// never sees.
pub fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFFB}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// A character that must be written out to be seen: a control character
/// (which can move the cursor, hide a line or recolour text) or one that
/// does not show at all. Line breaks and tabs are neither.
pub(crate) fn needs_a_mark(c: char) -> bool {
    !matches!(c, '\n' | '\t') && (c.is_control() || is_invisible(c))
}

fn push_mark(out: &mut String, c: char) {
    out.push_str(&format!("<U+{:04X}>", c as u32));
}

/// Text from the network or from a skill's files, safe to show: each control
/// character and each character that does not show at all is replaced with a
/// visible `<U+XXXX>`, so nothing a file holds goes unseen. Line breaks and
/// tabs stay.
pub fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if needs_a_mark(c) {
            push_mark(&mut out, c);
        } else {
            out.push(c);
        }
    }
    out
}

/// One file's text as a preview shows it, and how many characters were
/// written out as `<U+XXXX>` marks. It is [`printable`] with the two things
/// an ordinary text file may hold set apart: a byte-order mark at the very
/// start is left out (it is no part of what the file says, and every door
/// shows the text without it, neither raw nor as a mark), and the carriage
/// return of a Windows line ending is left alone.
fn shown_text(text: &str) -> (String, usize) {
    let text = without_leading_bom(text);
    let mut out = String::with_capacity(text.len());
    let mut marked = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let ordinary = c == '\r' && chars.peek().is_some_and(|next| *next == '\n');
        if needs_a_mark(c) && !ordinary {
            push_mark(&mut out, c);
            marked += 1;
        } else {
            out.push(c);
        }
    }
    (out, marked)
}

/// Characters no skill file may hold: the tag block, which spells out words
/// that never show, and the controls that override or isolate the direction
/// of text, which make a line read as something other than what it says.
/// Nothing a person writes for an AI tool to read needs them.
fn never_shown_honestly(c: char) -> bool {
    matches!(
        c,
        '\u{E0000}'..='\u{E007F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// One file as a preview shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownFile {
    pub path: String,
    /// `None` for a file that is not text. A character that would not show
    /// is written out as `<U+XXXX>`; the file itself is never changed.
    pub text: Option<String>,
    /// The text is longer than what is shown.
    pub cut: bool,
    /// How many characters of `text` were written out as marks.
    pub marked: usize,
}

/// Each file for showing before install, and whether all of it is shown.
pub fn preview_files(files: &[SkillFile]) -> Vec<ShownFile> {
    files
        .iter()
        .map(|f| {
            let whole = std::str::from_utf8(&f.bytes).ok();
            let end = whole.and_then(|t| t.char_indices().nth(PREVIEW_CHARS).map(|(at, _)| at));
            let (text, marked) = match whole.map(|t| shown_text(&t[..end.unwrap_or(t.len())])) {
                Some((text, marked)) => (Some(text), marked),
                None => (None, 0),
            };
            ShownFile {
                path: f.path.clone(),
                text,
                cut: end.is_some(),
                marked,
            }
        })
        .collect()
}

/// Files an AI tool reads as instructions or notes; they must be text a
/// person can be shown.
pub(crate) fn must_be_readable_text(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == "skill.md"
        || [".md", ".markdown", ".mdx", ".txt"]
            .iter()
            .any(|ending| lower.ends_with(ending))
}

/// Why this skill holds text the person could not be shown as it is, if it
/// does: a text file longer than a preview shows, a SKILL.md (or other
/// notes file) that is not valid UTF-8 and so shows as "not a text file",
/// or a text file with characters that spell words without showing or that
/// reorder the line. Such a skill must not be installed or taken in,
/// because an AI tool would read what the person never saw.
pub fn hidden_from_the_person(files: &[SkillFile]) -> Option<String> {
    for (file, shown) in files.iter().zip(preview_files(files)) {
        if shown.cut {
            return Some(format!(
                "{} is too long to show in full ({PREVIEW_CHARS} characters at most), so Wenlan \
                 refuses this skill",
                shown.path
            ));
        }
        if shown.text.is_none() && must_be_readable_text(&shown.path) {
            return Some(format!(
                "{} is not readable text (not valid UTF-8), so Wenlan refuses this skill",
                shown.path
            ));
        }
        let hidden = std::str::from_utf8(&file.bytes)
            .ok()
            .and_then(|text| text.chars().find(|c| never_shown_honestly(*c)));
        if let Some(c) = hidden {
            return Some(format!(
                "{} holds characters that do not show or that reorder the text (the first is \
                 U+{:04X}), so Wenlan refuses this skill",
                shown.path, c as u32
            ));
        }
    }
    None
}

#[path = "skill_registry_actions.rs"]
pub mod actions;

#[cfg(test)]
#[path = "skill_registry_tests.rs"]
mod tests;
