// SPDX-License-Identifier: Apache-2.0
//! Client-side session context shared by the CLI and the MCP server.
//!
//! Decides which AI session a write belongs to (`<tool>:<external_id>`) and
//! gathers the best-effort metadata the daemon records for it: working
//! directory, git repository, and where the tool keeps the transcript. Std
//! only, so this crate gains no dependency. Every lookup is best effort: a
//! missing git, an unreadable directory or a slow command yields `None`
//! fields, never an error, because recording a session must not fail the
//! write it rides on.

use crate::sessions::{SessionRef, UpsertSessionRequest};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Environment variable naming the session as `<tool>:<external_id>`.
pub const SESSION_ENV: &str = "WENLAN_SESSION";
/// Set by Codex CLI for the shell commands it runs.
pub const CODEX_THREAD_ENV: &str = "CODEX_THREAD_ID";

/// Total time the git lookups may take.
const GIT_BUDGET: Duration = Duration::from_millis(1500);
/// Entries examined while searching a Codex sessions tree.
const CODEX_WALK_ENTRY_LIMIT: usize = 5000;
/// Directory levels below `sessions/` the Codex search descends.
const CODEX_WALK_MAX_DEPTH: usize = 4;
/// Project directories examined by the Claude Code fallback scan.
const CLAUDE_SCAN_DIR_LIMIT: usize = 2000;

/// The outcome of resolving a session, with warnings for ignored values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedSession {
    pub session: Option<SessionRef>,
    pub warnings: Vec<String>,
}

/// Pick the session: `explicit` first, then `WENLAN_SESSION`, then
/// `CODEX_THREAD_ID` (as `codex:<id>`). A blank value counts as absent. An
/// invalid value is skipped with a warning and the next source is tried.
pub fn resolve_session(
    explicit: Option<&str>,
    env_session: Option<&str>,
    codex_thread_id: Option<&str>,
) -> ResolvedSession {
    let mut resolved = ResolvedSession::default();
    let candidates = [
        ("the session argument", explicit, false),
        (SESSION_ENV, env_session, false),
        (CODEX_THREAD_ENV, codex_thread_id, true),
    ];
    for (source, raw, is_codex_thread) in candidates {
        let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
            continue;
        };
        let parsed = if is_codex_thread {
            SessionRef::new("codex", raw)
        } else {
            SessionRef::parse(raw)
        };
        match parsed {
            Ok(session) => {
                resolved.session = Some(session);
                return resolved;
            }
            Err(reason) => resolved
                .warnings
                .push(format!("ignoring invalid session from {source}: {reason}")),
        }
    }
    resolved
}

/// [`resolve_session`] reading the process environment.
pub fn resolve_session_from_env(explicit: Option<&str>) -> ResolvedSession {
    resolve_session(
        explicit,
        std::env::var(SESSION_ENV).ok().as_deref(),
        std::env::var(CODEX_THREAD_ENV).ok().as_deref(),
    )
}

/// Where the process is running: working directory and git state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkContext {
    pub cwd: Option<String>,
    pub repo_root: Option<String>,
    pub repo_remote: Option<String>,
    pub repo_branch: Option<String>,
    pub repo_commit: Option<String>,
}

/// The process's [`WorkContext`], computed once. Blocking (runs `git`), so
/// call it from `spawn_blocking` in async code.
pub fn process_work_context() -> &'static WorkContext {
    static CONTEXT: OnceLock<WorkContext> = OnceLock::new();
    CONTEXT.get_or_init(|| match std::env::current_dir() {
        Ok(dir) => collect_work_context(&dir),
        Err(_) => WorkContext::default(),
    })
}

/// Gather the working directory and git state for `dir`.
pub fn collect_work_context(dir: &Path) -> WorkContext {
    let mut context = WorkContext {
        cwd: Some(dir.to_string_lossy().into_owned()),
        ..WorkContext::default()
    };
    let deadline = Instant::now() + GIT_BUDGET;
    let Some(root) = run_git(dir, &["rev-parse", "--show-toplevel"], deadline) else {
        return context;
    };
    context.repo_root = Some(root);
    context.repo_remote = run_git(dir, &["config", "--get", "remote.origin.url"], deadline)
        .map(|url| strip_url_credentials(&url));
    // `symbolic-ref` is empty on a detached HEAD, and works before the first commit.
    context.repo_branch = run_git(dir, &["symbolic-ref", "--short", "-q", "HEAD"], deadline);
    context.repo_commit = run_git(dir, &["rev-parse", "HEAD"], deadline);
    context
}

/// Run `git` in `dir` and return its trimmed, non-empty stdout, or `None` on
/// any failure or once `deadline` passes.
fn run_git(dir: &Path, args: &[&str], deadline: Instant) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                let out = out.trim();
                return (!out.is_empty()).then(|| out.to_string());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Remove `user:password@` from a URL-style remote so a token never leaves
/// the machine. scp-style remotes (`git@host:owner/repo`) carry no secret and
/// are returned unchanged.
pub fn strip_url_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    match authority.rfind('@') {
        Some(at) => format!("{scheme}://{}{tail}", &authority[at + 1..]),
        None => url.to_string(),
    }
}

/// Directories the transcript lookup reads from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptRoots {
    /// Claude Code config dir (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
    pub claude_dir: Option<PathBuf>,
    /// Codex home (`CODEX_HOME`, else `~/.codex`).
    pub codex_dir: Option<PathBuf>,
}

impl TranscriptRoots {
    /// Resolve the roots from `home` and the environment.
    pub fn from_env(home: Option<PathBuf>) -> Self {
        let from_var = |name: &str, default: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(default)))
        };
        Self {
            claude_dir: from_var("CLAUDE_CONFIG_DIR", ".claude"),
            codex_dir: from_var("CODEX_HOME", ".codex"),
        }
    }
}

/// Claude Code's project directory name for a working directory: every
/// character that is not an ASCII letter or digit becomes `-`.
pub fn claude_project_slug(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Where the tool keeps this session's transcript, only if the file exists.
pub fn find_transcript(
    session: &SessionRef,
    context: &WorkContext,
    roots: &TranscriptRoots,
) -> Option<String> {
    let path = match session.tool.as_str() {
        "claude-code" => find_claude_transcript(&session.external_id, context, roots)?,
        "codex" => find_codex_transcript(&session.external_id, roots)?,
        _ => return None,
    };
    Some(path.to_string_lossy().into_owned())
}

fn find_claude_transcript(
    id: &str,
    context: &WorkContext,
    roots: &TranscriptRoots,
) -> Option<PathBuf> {
    let projects = roots.claude_dir.as_ref()?.join("projects");
    let file = format!("{id}.jsonl");
    // Claude Code keys the directory by the directory it was started in,
    // which is usually this process's cwd or the repository root.
    for dir in [&context.cwd, &context.repo_root].into_iter().flatten() {
        let candidate = projects.join(claude_project_slug(dir)).join(&file);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // Started elsewhere (or a long path Claude Code shortens): scan.
    std::fs::read_dir(&projects)
        .ok()?
        .filter_map(Result::ok)
        .take(CLAUDE_SCAN_DIR_LIMIT)
        .map(|entry| entry.path().join(&file))
        .find(|candidate| candidate.is_file())
}

fn find_codex_transcript(id: &str, roots: &TranscriptRoots) -> Option<PathBuf> {
    let sessions = roots.codex_dir.as_ref()?.join("sessions");
    let mut budget = CODEX_WALK_ENTRY_LIMIT;
    walk_for_name(&sessions, id, 0, &mut budget)
}

/// Depth-first search, newest-named entries first (Codex nests by date).
fn walk_for_name(dir: &Path, needle: &str, depth: usize, budget: &mut usize) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    let mut subdirs = Vec::new();
    for entry in entries {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            subdirs.push(entry.path());
        } else if kind.is_file() && entry.file_name().to_string_lossy().contains(needle) {
            return Some(entry.path());
        }
    }
    if depth >= CODEX_WALK_MAX_DEPTH {
        return None;
    }
    subdirs
        .into_iter()
        .find_map(|sub| walk_for_name(&sub, needle, depth + 1, budget))
}

/// The `POST /api/sessions` body for `session`.
pub fn build_upsert_request(
    session: &SessionRef,
    context: &WorkContext,
    roots: &TranscriptRoots,
) -> UpsertSessionRequest {
    UpsertSessionRequest {
        session: session.to_string(),
        title: None,
        cwd: context.cwd.clone(),
        repo_root: context.repo_root.clone(),
        repo_remote: context.repo_remote.clone(),
        repo_branch: context.repo_branch.clone(),
        repo_commit: context.repo_commit.clone(),
        transcript_path: find_transcript(session, context, roots),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const CLAUDE_ID: &str = "00893aaf-19fa-41d2-8238-13269b9b3ca0";

    fn session(raw: &str) -> SessionRef {
        SessionRef::parse(raw).unwrap()
    }

    #[test]
    fn explicit_beats_env_beats_codex_thread() {
        let all = resolve_session(Some("claude-code:a"), Some("cursor:b"), Some("c"));
        assert_eq!(all.session, Some(session("claude-code:a")));
        let env = resolve_session(None, Some("cursor:b"), Some("c"));
        assert_eq!(env.session, Some(session("cursor:b")));
        let codex = resolve_session(None, None, Some("019a-bc"));
        assert_eq!(codex.session, Some(session("codex:019a-bc")));
        assert!(codex.warnings.is_empty());
        assert_eq!(
            resolve_session(None, None, None),
            ResolvedSession::default()
        );
    }

    #[test]
    fn blank_values_are_absent_without_a_warning() {
        let resolved = resolve_session(Some("  "), Some(""), Some("\n"));
        assert_eq!(resolved, ResolvedSession::default());
    }

    #[test]
    fn invalid_values_warn_and_fall_through() {
        let resolved = resolve_session(Some("nocolon"), Some("Bad:id"), Some("has:colon"));
        assert_eq!(resolved.session, None);
        assert_eq!(resolved.warnings.len(), 3);
        assert!(resolved.warnings[0].contains("session argument"));
        assert!(resolved.warnings[1].contains(SESSION_ENV));
        assert!(resolved.warnings[2].contains(CODEX_THREAD_ENV));

        let fallback = resolve_session(Some("nocolon"), Some("claude-code:ok"), None);
        assert_eq!(fallback.session, Some(session("claude-code:ok")));
        assert_eq!(fallback.warnings.len(), 1);
    }

    #[test]
    fn slug_replaces_every_non_alphanumeric_character() {
        assert_eq!(
            claude_project_slug("/Users/me/.dotfiles/my_repo"),
            "-Users-me--dotfiles-my-repo"
        );
        assert_eq!(claude_project_slug(r"C:\Users\me\repo"), "C--Users-me-repo");
    }

    #[test]
    fn credentials_are_stripped_from_remote_urls() {
        for (raw, expected) in [
            (
                "https://user:tok@github.com/o/r.git",
                "https://github.com/o/r.git",
            ),
            (
                "https://tok@github.com/o/r.git",
                "https://github.com/o/r.git",
            ),
            ("ssh://git@github.com/o/r.git", "ssh://github.com/o/r.git"),
            ("https://github.com/o/r.git", "https://github.com/o/r.git"),
            ("git@github.com:o/r.git", "git@github.com:o/r.git"),
            (
                "https://github.com/o/r@v1.git",
                "https://github.com/o/r@v1.git",
            ),
        ] {
            assert_eq!(strip_url_credentials(raw), expected, "{raw}");
        }
    }

    #[test]
    fn claude_transcript_found_by_cwd_slug_only_when_the_file_exists() {
        let home = TempDir::new().unwrap();
        let roots = TranscriptRoots {
            claude_dir: Some(home.path().join(".claude")),
            codex_dir: None,
        };
        let context = WorkContext {
            cwd: Some("/work/my.repo".into()),
            ..WorkContext::default()
        };
        let sess = session(&format!("claude-code:{CLAUDE_ID}"));
        assert_eq!(find_transcript(&sess, &context, &roots), None);

        let dir = home.path().join(".claude/projects/-work-my-repo");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{CLAUDE_ID}.jsonl"));
        std::fs::write(&file, "{}").unwrap();
        assert_eq!(
            find_transcript(&sess, &context, &roots),
            Some(file.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn claude_transcript_falls_back_to_the_repo_root_then_a_scan() {
        let home = TempDir::new().unwrap();
        let roots = TranscriptRoots {
            claude_dir: Some(home.path().join(".claude")),
            codex_dir: None,
        };
        let sess = session(&format!("claude-code:{CLAUDE_ID}"));
        let elsewhere = home.path().join(".claude/projects/-somewhere-else");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let file = elsewhere.join(format!("{CLAUDE_ID}.jsonl"));
        std::fs::write(&file, "{}").unwrap();
        let expected = Some(file.to_string_lossy().into_owned());

        let by_root = WorkContext {
            cwd: Some("/work/sub".into()),
            repo_root: Some("/somewhere/else".into()),
            ..WorkContext::default()
        };
        assert_eq!(find_transcript(&sess, &by_root, &roots), expected);
        // Neither cwd nor root matches: the bounded scan still finds it.
        let lost = WorkContext {
            cwd: Some("/nowhere".into()),
            ..WorkContext::default()
        };
        assert_eq!(find_transcript(&sess, &lost, &roots), expected);
    }

    #[test]
    fn codex_transcript_found_by_id_in_the_dated_tree() {
        let home = TempDir::new().unwrap();
        let roots = TranscriptRoots {
            claude_dir: None,
            codex_dir: Some(home.path().join(".codex")),
        };
        let id = "019a1b2c-0000-7000-8000-abcdef012345";
        let sess = session(&format!("codex:{id}"));
        let context = WorkContext::default();
        assert_eq!(find_transcript(&sess, &context, &roots), None);

        let day = home.path().join(".codex/sessions/2026/10/20");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-2026-10-20T01-00-00-other.jsonl"), "{}").unwrap();
        let file = day.join(format!("rollout-2026-10-20T02-00-00-{id}.jsonl"));
        std::fs::write(&file, "{}").unwrap();
        assert_eq!(
            find_transcript(&sess, &context, &roots),
            Some(file.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn codex_walk_is_bounded_in_depth() {
        let home = TempDir::new().unwrap();
        let roots = TranscriptRoots {
            claude_dir: None,
            codex_dir: Some(home.path().join(".codex")),
        };
        let context = WorkContext::default();
        let deep = home.path().join(".codex/sessions/a/b/c/d/e");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("rollout-deepid.jsonl"), "{}").unwrap();
        assert_eq!(
            find_transcript(&session("codex:deepid"), &context, &roots),
            None
        );
        let shallow = home.path().join(".codex/sessions/a/b/c/d");
        std::fs::write(shallow.join("rollout-okid.jsonl"), "{}").unwrap();
        assert!(find_transcript(&session("codex:okid"), &context, &roots).is_some());
    }

    #[test]
    fn unknown_tools_have_no_transcript() {
        assert_eq!(
            find_transcript(
                &session("cursor:abc"),
                &WorkContext::default(),
                &TranscriptRoots::default()
            ),
            None
        );
    }

    #[test]
    fn upsert_request_carries_context_and_the_canonical_name() {
        let home = TempDir::new().unwrap();
        let roots = TranscriptRoots {
            claude_dir: Some(home.path().join(".claude")),
            codex_dir: None,
        };
        let context = WorkContext {
            cwd: Some("/w".into()),
            repo_root: Some("/w".into()),
            repo_remote: Some("https://github.com/o/r.git".into()),
            repo_branch: Some("main".into()),
            repo_commit: Some("abc123".into()),
        };
        let request = build_upsert_request(&session("claude-code:x1"), &context, &roots);
        assert_eq!(request.session, "claude-code:x1");
        assert_eq!(request.cwd.as_deref(), Some("/w"));
        assert_eq!(request.repo_branch.as_deref(), Some("main"));
        assert_eq!(request.transcript_path, None);
    }

    #[test]
    fn work_context_reads_git_state_and_strips_credentials() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let repo = TempDir::new().unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "trunk"]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://user:secret@example.com/o/r.git",
        ]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);

        let context = collect_work_context(repo.path());
        assert_eq!(context.repo_branch.as_deref(), Some("trunk"));
        assert_eq!(
            context.repo_remote.as_deref(),
            Some("https://example.com/o/r.git")
        );
        assert_eq!(context.repo_commit.as_deref().map(str::len), Some(40));
        assert!(context.repo_root.is_some());
        assert!(!format!("{context:?}").contains("secret"));
    }

    #[test]
    fn work_context_always_reports_the_cwd() {
        let dir = TempDir::new().unwrap();
        let context = collect_work_context(dir.path());
        assert_eq!(
            context.cwd.as_deref(),
            Some(dir.path().to_string_lossy().as_ref())
        );
    }
}
