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
use std::path::{Component, Path, PathBuf};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillHit {
    /// `owner/repo`.
    pub source: String,
    /// The id skills.sh lists; not always the folder name in the repo.
    pub skill_id: String,
    pub name: String,
    pub installs: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchVia {
    #[serde(rename = "skills.sh")]
    SkillsSh,
    #[serde(rename = "wenlan.app")]
    WenlanApp,
}

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

fn valid_source(source: &str) -> bool {
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    /// At least one check warned, or no check has run yet.
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuditLine {
    pub provider: String,
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafetyReport {
    pub verdict: Verdict,
    pub audits: Vec<AuditLine>,
}

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

/// The `name:` in a SKILL.md's front matter.
pub fn front_matter_field(skill_md: &str, field: &str) -> Option<String> {
    let rest = skill_md.strip_prefix("---")?;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Scope {
    /// Every Space.
    All,
    /// Only while working in this Space.
    Space { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    /// Folder name, in the Wenlan folder and in each tool's folder.
    pub name: String,
    pub source: String,
    pub skill_id: String,
    pub repo_path: String,
    pub commit: String,
    pub hash: String,
    pub verdict: Verdict,
    pub scope: Scope,
    pub description: Option<String>,
    pub installed_at: i64,
    pub updated_at: i64,
}

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
    let (front, body) = match author.strip_prefix("---").and_then(|rest| {
        rest.find("\n---").map(|end| {
            let close = 3 + end + 4;
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
    for file in &mut files {
        if file.path == "SKILL.md" {
            let text = String::from_utf8_lossy(&file.bytes).to_string();
            file.bytes = compose_skill_md(&text, &record.scope, &additions).into_bytes();
        }
    }
    Ok(files)
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
    for tool in tool_dirs {
        let target = tool.join(name);
        if target.join(MANAGED_MARKER).is_file() {
            std::fs::remove_dir_all(&target).map_err(io_err(&target))?;
        }
    }
    std::fs::remove_dir_all(&dir).map_err(io_err(&dir))
}

/// One skill whose author published a change since it was installed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    pub name: String,
    /// "updated", "needs_look" (the new version did not pass every check),
    /// or "unchanged".
    pub outcome: String,
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
    let head = head_commit(http, endpoints, &record.source).await?;
    if head == record.commit {
        return Ok(outcome("unchanged"));
    }
    let fetched = fetch_skill(
        http,
        endpoints,
        &record.source,
        &record.skill_id,
        &head,
        Some(&record.repo_path),
    )
    .await?;
    if fetched.hash == record.hash {
        let mut moved = record.clone();
        moved.commit = head;
        write_record(&dir, &moved)?;
        return Ok(outcome("unchanged"));
    }
    let report = safety(http, endpoints, &record.source, &record.skill_id).await?;
    if report.verdict != Verdict::Pass {
        return Ok(outcome("needs_look"));
    }
    store_skill(root, &fetched, report.verdict, record.scope.clone(), now)?;
    Ok(outcome("updated"))
}

// ── Skills already on this computer ─────────────────────────────────────
//
// Folders the person (or another installer) put in an AI tool's skills
// folder. Importing copies them into the Wenlan folder; the original is
// never changed, so the tool that had it keeps using it, and the copy can
// reach the other tools.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkill {
    pub name: String,
    /// The folder it was found in.
    pub path: String,
    pub description: Option<String>,
}

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
            if !dir.is_dir()
                || dir.join(MANAGED_MARKER).exists()
                || held.contains(&name)
                || out.iter().any(|s| s.name == name)
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

#[cfg(test)]
#[path = "skill_registry_tests.rs"]
mod tests;
