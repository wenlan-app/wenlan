// SPDX-License-Identifier: Apache-2.0
//! Keeping a wiki folder that Wenlan put into a repository in step with the
//! code, from a plain checkout and without Wenlan's database: the GitHub
//! Actions route of the daily run (`wenlan wiki refresh`).
//!
//! A publish writes [`CODE_FILE`] beside the pages: for each page file, the
//! code lines it relies on and their anchors, taken from the default branch
//! the pull request was built on. From a later checkout, [`check`] finds each
//! span again:
//!
//! - unchanged: nothing to do;
//! - moved: the lines are the same, only their place changed, and
//!   [`apply_moves`] rewrites the line numbers without a model;
//! - changed or gone: the page needs rewriting. An AI tool edits those page
//!   files, then [`reanchor`] reads the `repo://` lines it left in each
//!   page's frontmatter, fingerprints them in the current code and rebuilds
//!   the `## Code` list.
//!
//! The OKF export marker beside the pages is left alone on purpose: its
//! digests are what the local publish last wrote, so a page changed here is
//! seen there as changed in the repository and brought back into the Space
//! before the next publish.

use crate::code_anchor::{self, CodeAnchor, LineSpan, Relocation};
use crate::error::WenlanError;
use crate::repo_wiki::{code_footer, git, run};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

/// The code manifest, in the wiki folder.
pub const CODE_FILE: &str = ".wenlan-code.json";
const MANIFEST_VERSION: u32 = 1;

static REPO_SOURCE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r##"repo://([^"#\s]+)#L(\d+)(?:-L(\d+))?"##).expect("valid regex")
});

/// What [`CODE_FILE`] holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CodeManifest {
    pub version: u32,
    /// The commit the spans were read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Page file (relative to the wiki folder, `/` separated) to its code.
    pub pages: BTreeMap<String, ManifestPage>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestPage {
    /// The page's id in Wenlan.
    pub id: String,
    pub code: Vec<ManifestSpan>,
    /// SHA-256 of the page file as `wenlan wiki refresh` last wrote it. A
    /// file that still matches was changed by the refresh, not by a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written: Option<String>,
}

/// SHA-256 of a page file's text, as [`ManifestPage::written`] records it.
pub fn file_digest(text: &str) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
}

/// Whether the page file `file` (relative to the wiki folder), reading
/// `text`, is as `wenlan wiki refresh` wrote it, by the manifest `manifest`.
pub fn written_by_refresh(manifest: &str, file: &str, text: &str) -> bool {
    serde_json::from_str::<CodeManifest>(manifest)
        .ok()
        .and_then(|manifest| manifest.pages.get(file)?.written.clone())
        .is_some_and(|written| written == file_digest(text))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestSpan {
    /// Repository-relative path.
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub anchor: CodeAnchor,
}

/// Where one span of a page is in the current code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SpanState {
    Unchanged,
    Moved {
        start: u32,
        end: u32,
    },
    Changed {
        start: u32,
        end: u32,
    },
    /// The file is gone, or the lines cannot be found in it.
    Missing,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SpanCheck {
    pub path: String,
    pub start: u32,
    pub end: u32,
    #[serde(flatten)]
    pub state: SpanState,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PageCheck {
    /// Page file, relative to the wiki folder.
    pub file: String,
    pub id: String,
    pub spans: Vec<SpanCheck>,
}

impl PageCheck {
    /// Some code the page relies on changed or is gone.
    pub fn needs_rewrite(&self) -> bool {
        self.spans
            .iter()
            .any(|span| matches!(span.state, SpanState::Changed { .. } | SpanState::Missing))
    }

    /// Only line numbers moved.
    pub fn only_moved(&self) -> bool {
        !self.needs_rewrite()
            && self
                .spans
                .iter()
                .any(|span| matches!(span.state, SpanState::Moved { .. }))
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct WikiCheck {
    /// The commit the manifest was written at.
    pub base: Option<String>,
    /// Every page with at least one span that is not unchanged.
    pub pages: Vec<PageCheck>,
    /// Pages whose code is all where it was.
    pub unchanged: usize,
}

impl WikiCheck {
    pub fn to_rewrite(&self) -> impl Iterator<Item = &PageCheck> {
        self.pages.iter().filter(|page| page.needs_rewrite())
    }

    pub fn moved(&self) -> impl Iterator<Item = &PageCheck> {
        self.pages.iter().filter(|page| page.only_moved())
    }
}

pub fn read_manifest(wiki_dir: &Path) -> Result<CodeManifest, WenlanError> {
    let path = wiki_dir.join(CODE_FILE);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WenlanError::Validation(format!(
                "{} is missing; publish the wiki from Wenlan once (`wenlan wiki publish`) \
                 and merge the pull request first",
                path.display()
            ))
        } else {
            error.into()
        }
    })?;
    let manifest: CodeManifest = serde_json::from_str(&text)?;
    if manifest.version != MANIFEST_VERSION {
        return Err(WenlanError::Validation(format!(
            "{} is version {}; this Wenlan reads version {MANIFEST_VERSION}. Update Wenlan.",
            path.display(),
            manifest.version
        )));
    }
    Ok(manifest)
}

pub fn write_manifest(wiki_dir: &Path, manifest: &CodeManifest) -> Result<(), WenlanError> {
    let mut text = serde_json::to_string_pretty(manifest)?;
    text.push('\n');
    std::fs::write(wiki_dir.join(CODE_FILE), text)?;
    Ok(())
}

/// The manifest for pages as a publish writes them: `pages` maps each page
/// file to its id and its spans with the text of each file at `base`.
pub fn build_manifest<'a>(
    base: Option<String>,
    pages: impl IntoIterator<Item = (String, String, Vec<(String, u32, u32)>)>,
    file_text: impl Fn(&str) -> Option<&'a str>,
) -> CodeManifest {
    let mut manifest = CodeManifest {
        version: MANIFEST_VERSION,
        base,
        pages: BTreeMap::new(),
    };
    for (file, id, spans) in pages {
        let code = spans
            .into_iter()
            .filter_map(|(path, start, end)| {
                let anchor =
                    code_anchor::anchor(file_text(&path)?, LineSpan { start, end }).ok()?;
                Some(ManifestSpan {
                    path,
                    start,
                    end,
                    anchor,
                })
            })
            .collect();
        manifest.pages.insert(
            file,
            ManifestPage {
                id,
                code,
                written: None,
            },
        );
    }
    manifest
}

fn read_code(repo: &Path, path: &str) -> Option<String> {
    if Path::new(path)
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }
    std::fs::read_to_string(repo.join(path)).ok()
}

/// Find every page's code again in the working tree at `repo`.
pub fn check(repo: &Path, folder: &str) -> Result<WikiCheck, WenlanError> {
    let manifest = read_manifest(&repo.join(folder))?;
    let mut texts: BTreeMap<&str, Option<String>> = BTreeMap::new();
    let mut out = WikiCheck {
        base: manifest.base.clone(),
        ..WikiCheck::default()
    };
    for (file, page) in &manifest.pages {
        let mut spans = Vec::new();
        for span in &page.code {
            let text = texts
                .entry(span.path.as_str())
                .or_insert_with(|| read_code(repo, &span.path));
            let hint = Some(LineSpan {
                start: span.start,
                end: span.end,
            });
            let state = match text.as_deref() {
                None => SpanState::Missing,
                Some(text) => match code_anchor::relocate(text, hint, &span.anchor) {
                    Relocation::Unchanged { .. } => SpanState::Unchanged,
                    Relocation::Moved { span } => SpanState::Moved {
                        start: span.start,
                        end: span.end,
                    },
                    Relocation::Changed { span } => SpanState::Changed {
                        start: span.start,
                        end: span.end,
                    },
                    Relocation::Unresolved => SpanState::Missing,
                },
            };
            spans.push(SpanCheck {
                path: span.path.clone(),
                start: span.start,
                end: span.end,
                state,
            });
        }
        if spans.iter().all(|span| span.state == SpanState::Unchanged) {
            out.unchanged += 1;
        } else {
            out.pages.push(PageCheck {
                file: file.clone(),
                id: page.id.clone(),
                spans,
            });
        }
    }
    Ok(out)
}

/// Split a page file into its frontmatter (without the `---` fences) and
/// body. A file without frontmatter is all body.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    match rest.find("\n---\n") {
        Some(end) => (Some(&rest[..end + 1]), &rest[end + 5..]),
        None => (None, text),
    }
}

/// The `repo://` spans named in a page's frontmatter, in order of first
/// mention, without repeats.
pub fn frontmatter_spans(text: &str) -> Vec<(String, u32, u32)> {
    let Some(frontmatter) = split_frontmatter(text).0 else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for cap in REPO_SOURCE_RE.captures_iter(frontmatter) {
        let path = cap[1].to_owned();
        let Ok(start) = cap[2].parse::<u32>() else {
            continue;
        };
        let end = cap
            .get(3)
            .and_then(|end| end.as_str().parse::<u32>().ok())
            .unwrap_or(start);
        if seen.insert((path.clone(), start, end)) {
            out.push((path, start, end));
        }
    }
    out
}

/// The body without the `## Code` list a publish closes each page with.
fn strip_code_footer(body: &str) -> &str {
    let trimmed = body.trim_end();
    let Some(at) = trimmed.rfind("\n## Code\n") else {
        return body;
    };
    let after = &trimmed[at + "\n## Code\n".len()..];
    if after
        .lines()
        .all(|line| line.trim().is_empty() || line.starts_with("- [`"))
    {
        &trimmed[..at]
    } else {
        body
    }
}

/// `text` (a whole page file) with its `repo://` sources and `## Code` list
/// naming `spans` instead.
pub fn rewrite_page_code(text: &str, spans: &[(String, u32, u32)], folder: &str) -> String {
    let (frontmatter, body) = split_frontmatter(text);
    let mut sorted: Vec<&(String, u32, u32)> = spans.iter().collect();
    sorted.sort();
    sorted.dedup();
    let code_lines: Vec<String> = sorted
        .iter()
        .map(|(path, start, end)| {
            format!(
                "  - {{resource: {}}}",
                crate::export::provenance::yaml_quoted(&format!("repo://{path}#L{start}-L{end}"))
            )
        })
        .collect();
    let mut out = String::new();
    if let Some(frontmatter) = frontmatter {
        out.push_str("---\n");
        let lines: Vec<&str> = frontmatter.lines().collect();
        let mut kept: Vec<String> = Vec::new();
        let mut sources_at = None;
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            if line == "sources:" {
                sources_at = Some(kept.len());
                kept.push(line.to_owned());
                i += 1;
                while i < lines.len() && lines[i].starts_with("  ") {
                    if !lines[i].contains("repo://") {
                        kept.push(lines[i].to_owned());
                    }
                    i += 1;
                }
                continue;
            }
            kept.push(line.to_owned());
            i += 1;
        }
        match sources_at {
            Some(at) => {
                let mut end = at + 1;
                while end < kept.len() && kept[end].starts_with("  ") {
                    end += 1;
                }
                kept.splice(end..end, code_lines.iter().cloned());
                if end == at + 1 && code_lines.is_empty() {
                    kept.remove(at);
                }
            }
            None if !code_lines.is_empty() => {
                kept.push("sources:".to_owned());
                kept.extend(code_lines.iter().cloned());
            }
            None => {}
        }
        for line in kept {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("---\n");
    }
    let body = strip_code_footer(body);
    if sorted.is_empty() {
        out.push_str(body.trim_end());
    } else {
        out.push_str(&code_footer(
            body,
            sorted
                .iter()
                .map(|(path, start, end)| (path.as_str(), *start, *end)),
            folder,
        ));
        // The export closes a page file with one more newline after the
        // body, so a refreshed page reads byte for byte like a published one.
        out.push('\n');
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Rewrite the line numbers of every page whose code only moved, in its file
/// and in the manifest. Returns how many pages changed.
pub fn apply_moves(repo: &Path, folder: &str, found: &WikiCheck) -> Result<usize, WenlanError> {
    let wiki_dir = repo.join(folder);
    let mut manifest = read_manifest(&wiki_dir)?;
    let mut changed = 0;
    for page in found.moved() {
        let Some(entry) = manifest.pages.get_mut(&page.file) else {
            continue;
        };
        for (span, now) in entry.code.iter_mut().zip(&page.spans) {
            if let SpanState::Moved { start, end } = now.state {
                span.start = start;
                span.end = end;
            }
        }
        let spans: Vec<(String, u32, u32)> = entry
            .code
            .iter()
            .map(|span| (span.path.clone(), span.start, span.end))
            .collect();
        let path = wiki_dir.join(&page.file);
        let text = std::fs::read_to_string(&path)?;
        let next = rewrite_page_code(&text, &spans, folder);
        if next != text {
            std::fs::write(&path, &next)?;
        }
        entry.written = Some(file_digest(&next));
        changed += 1;
    }
    if changed > 0 {
        write_manifest(&wiki_dir, &manifest)?;
    }
    Ok(changed)
}

/// What [`reanchor`] did with one page file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Reanchored {
    /// Its code list and anchors now match the code.
    Updated { file: String },
    /// A `repo://` line names a file or lines that do not exist: the page
    /// is left for a person.
    Broken { file: String, reason: String },
}

/// After an AI tool edited `files` (relative to the wiki folder): read the
/// `repo://` lines each one names, fingerprint them in the working tree,
/// rebuild its `## Code` list and record it in the manifest. `base` is the
/// commit those spans now belong to.
pub fn reanchor(
    repo: &Path,
    folder: &str,
    files: &[String],
    base: Option<String>,
) -> Result<Vec<Reanchored>, WenlanError> {
    let wiki_dir = repo.join(folder);
    let mut manifest = read_manifest(&wiki_dir)?;
    let mut out = Vec::new();
    for file in files {
        let Some(entry) = manifest.pages.get_mut(file) else {
            continue;
        };
        let path = wiki_dir.join(file);
        let text = std::fs::read_to_string(&path)?;
        let spans = frontmatter_spans(&text);
        let mut code = Vec::new();
        let mut broken = None;
        for (code_path, start, end) in &spans {
            let anchored = read_code(repo, code_path)
                .ok_or_else(|| format!("{code_path} does not exist"))
                .and_then(|source| {
                    code_anchor::anchor(
                        &source,
                        LineSpan {
                            start: *start,
                            end: *end,
                        },
                    )
                    .map_err(|error| format!("{code_path} lines {start}-{end}: {error}"))
                });
            match anchored {
                Ok(anchor) => code.push(ManifestSpan {
                    path: code_path.clone(),
                    start: *start,
                    end: *end,
                    anchor,
                }),
                Err(reason) => {
                    broken = Some(reason);
                    break;
                }
            }
        }
        if spans.is_empty() {
            broken = Some("it names no code lines".to_owned());
        }
        if let Some(reason) = broken {
            out.push(Reanchored::Broken {
                file: file.clone(),
                reason,
            });
            continue;
        }
        let next = rewrite_page_code(&text, &spans, folder);
        if next != text {
            std::fs::write(&path, &next)?;
        }
        entry.code = code;
        entry.written = Some(file_digest(&next));
        out.push(Reanchored::Updated { file: file.clone() });
    }
    manifest.base = base;
    write_manifest(&wiki_dir, &manifest)?;
    Ok(out)
}

/// The commit checked out at `repo`.
pub fn head_commit(repo: &Path) -> Option<String> {
    let mut command = git(repo);
    command.args(["rev-parse", "HEAD"]);
    run(command, "git rev-parse HEAD").ok()
}

/// The prompt for the AI tool that rewrites the pages in `found` whose code
/// changed.
pub fn rewrite_prompt(folder: &str, found: &WikiCheck) -> String {
    let mut out = format!(
        "You keep the wiki in `{folder}/` of this repository in step with the code. The code \
         some pages rely on has changed. Update only the page files listed below, nothing else.\n\n\
         For each page:\n\
         1. Read the page, then the code it names, as it is now.\n\
         2. Change only the sentences the code change made wrong or incomplete. Keep the rest \
         word for word, and keep the page's title, language and style.\n\
         3. In the frontmatter, the `sources:` list names the code the page relies on as lines \
         like `  - {{resource: \"repo://src/lib.rs#L10-L20\"}}`. Make that list name the lines \
         the page relies on now: fix the numbers, drop lines that are gone, add lines the new \
         text relies on. Leave every other frontmatter line as it is.\n\
         4. Do not edit the `## Code` list at the end of the page; it is rebuilt from the \
         frontmatter.\n\n"
    );
    if let Some(base) = &found.base {
        out.push_str(&format!(
            "The pages were last right at commit {base}; `git diff {base} HEAD -- <path>` shows \
             what changed in a file since.\n\n"
        ));
    }
    out.push_str("Pages:\n");
    for page in found.to_rewrite() {
        out.push_str(&format!("- `{folder}/{}`\n", page.file));
        for span in &page.spans {
            let now = match span.state {
                SpanState::Unchanged => "unchanged".to_owned(),
                SpanState::Moved { start, end } => format!("moved to lines {start}-{end}"),
                SpanState::Changed { start, end } => {
                    format!("changed, now around lines {start}-{end}")
                }
                SpanState::Missing => "gone or no longer found".to_owned(),
            };
            out.push_str(&format!(
                "  - `{}` lines {}-{}: {now}\n",
                span.path, span.start, span.end
            ));
        }
    }
    out
}

/// The branch a refresh opens its pull request from. Not the publish's
/// `wenlan/wiki`, which a publish rebuilds from the Space each time.
pub const REFRESH_BRANCH: &str = "wenlan/wiki-refresh";

/// Where the GitHub Actions workflow goes.
pub const WORKFLOW_PATH: &str = ".github/workflows/wenlan-wiki.yml";

/// What a refresh's Claude Code may do without asking: read, edit files and
/// read git history. Which files it may change is checked afterwards.
const CLAUDE_REFRESH_TOOLS: &[&str] = &[
    "Read",
    "Edit",
    "Glob",
    "Grep",
    "Bash(git diff:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
];

/// Program arguments for the AI tool that rewrites pages with `prompt`.
pub fn refresh_agent_args(
    agent: wenlan_types::repo_wiki::RepoWikiAgent,
    prompt: &str,
) -> Vec<String> {
    use wenlan_types::repo_wiki::RepoWikiAgent;
    match agent {
        RepoWikiAgent::Claude => {
            let mut args = vec![
                "-p".to_owned(),
                prompt.to_owned(),
                "--allowedTools".to_owned(),
            ];
            args.extend(CLAUDE_REFRESH_TOOLS.iter().map(|tool| (*tool).to_owned()));
            args
        }
        RepoWikiAgent::Codex => vec![
            "exec".into(),
            "--skip-git-repo-check".into(),
            "--sandbox".into(),
            "workspace-write".into(),
            prompt.to_owned(),
        ],
    }
}

/// The GitHub Actions workflow that refreshes the wiki in `folder` every day
/// at `utc_hour` with `agent`, and opens a pull request when a page changed.
pub fn workflow(
    folder: &str,
    agent: wenlan_types::repo_wiki::RepoWikiAgent,
    utc_hour: u8,
) -> String {
    use wenlan_types::repo_wiki::RepoWikiAgent;
    let (tool, name, install, key_help, env) = match agent {
        RepoWikiAgent::Claude => (
            "claude",
            "Claude Code",
            "npm install --global @anthropic-ai/claude-code",
            "CLAUDE_CODE_OAUTH_TOKEN (run `claude setup-token` to use your Claude plan)\n\
             #   or ANTHROPIC_API_KEY (a Claude API key).",
            "          CLAUDE_CODE_OAUTH_TOKEN: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}\n\
             \x20         ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}\n",
        ),
        RepoWikiAgent::Codex => (
            "codex",
            "Codex",
            "npm install --global @openai/codex",
            "OPENAI_API_KEY (an OpenAI API key).",
            "          CODEX_API_KEY: ${{ secrets.OPENAI_API_KEY }}\n",
        ),
    };
    format!(
        "# Written by Wenlan (https://wenlan.app). Every day, checks whether the code the\n\
         # pages in `{folder}/` rely on changed, has the AI tool fix the pages whose code\n\
         # changed, and opens a pull request from `{REFRESH_BRANCH}`. Nothing changed: no\n\
         # model call and no pull request.\n\
         #\n\
         # Needs, once, in the repository's settings:\n\
         # - Secrets and variables > Actions: {key_help}\n\
         # - Actions > General: allow GitHub Actions to create pull requests.\n\
         name: Wenlan wiki\n\
         on:\n\
         \x20 schedule:\n\
         \x20   - cron: \"17 {utc_hour} * * *\"\n\
         \x20 workflow_dispatch:\n\
         permissions:\n\
         \x20 contents: write\n\
         \x20 pull-requests: write\n\
         concurrency:\n\
         \x20 group: wenlan-wiki\n\
         jobs:\n\
         \x20 refresh:\n\
         \x20   runs-on: ubuntu-latest\n\
         \x20   timeout-minutes: 30\n\
         \x20   steps:\n\
         \x20     - uses: actions/checkout@v6\n\
         \x20       with:\n\
         \x20         fetch-depth: 0\n\
         \x20     - name: Install Wenlan and {name}\n\
         \x20       run: |\n\
         \x20         curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash\n\
         \x20         echo \"$HOME/.wenlan/bin\" >> \"$GITHUB_PATH\"\n\
         \x20         {install}\n\
         \x20     - name: Bring the wiki up to date\n\
         \x20       env:\n\
         {env}\
         \x20         GH_TOKEN: ${{{{ github.token }}}}\n\
         \x20         WENLAN_NO_AUTOSTART: \"1\"\n\
         \x20       run: wenlan wiki refresh --folder {folder} --agent {agent_arg} --pull-request\n",
        agent_arg = tool,
    )
}

#[cfg(test)]
#[path = "repo_wiki_ci_test.rs"]
mod tests;
