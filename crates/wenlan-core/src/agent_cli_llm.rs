// SPDX-License-Identifier: Apache-2.0
//! The person's own AI tool (Claude Code or Codex) as a model Wenlan can ask,
//! for work Wenlan's own models do poorly. It runs non-interactively, with no
//! tools, outside any project, keeps no conversation of its own, and spends
//! that tool's own quota.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use wenlan_types::repo_wiki::RepoWikiAgent;

/// Longest one answer may take before the tool is stopped.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Claude Code (`claude -p`, its small model) or Codex (`codex exec`).
pub struct AgentCliProvider {
    agent: RepoWikiAgent,
    program: PathBuf,
    /// The other tool, when both are installed: asked once when the first
    /// fails (for example a signed-out Claude Code), so one expired sign-in
    /// does not end the night's work.
    fallback: Option<(RepoWikiAgent, PathBuf)>,
}

impl AgentCliProvider {
    /// The first of Claude Code and Codex installed on this computer, with
    /// the other as its fallback.
    pub fn find() -> Option<Self> {
        let mut found = [RepoWikiAgent::Claude, RepoWikiAgent::Codex]
            .into_iter()
            .filter_map(|agent| {
                crate::repo_wiki::find_agent(agent).map(|program| (agent, program))
            });
        let (agent, program) = found.next()?;
        Some(Self {
            agent,
            program,
            fallback: found.next(),
        })
    }

    /// The program's arguments; the prompt goes on standard input.
    fn args(agent: RepoWikiAgent, answer_file: &std::path::Path) -> Vec<String> {
        match agent {
            // Codex has no way to turn its tools off; the read-only sandbox
            // keeps it from changing anything.
            RepoWikiAgent::Codex => vec![
                "exec".into(),
                "--skip-git-repo-check".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--output-last-message".into(),
                answer_file.to_string_lossy().into_owned(),
                "-".into(),
            ],
            _ => vec![
                "-p".into(),
                "--model".into(),
                "haiku".into(),
                "--no-session-persistence".into(),
                // No tools, no MCP servers, no settings (and so no hooks):
                // the model only answers from the prompt.
                "--tools".into(),
                String::new(),
                "--strict-mcp-config".into(),
                "--setting-sources".into(),
                String::new(),
                "--output-format".into(),
                "text".into(),
            ],
        }
    }
}

#[async_trait]
impl LlmProvider for AgentCliProvider {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        match run_agent(self.agent, &self.program, &request).await {
            Err(LlmError::InferenceFailed(first)) => match &self.fallback {
                Some((agent, program)) => run_agent(*agent, program, &request)
                    .await
                    .map_err(|second| LlmError::InferenceFailed(format!("{first}; then {second}"))),
                None => Err(LlmError::InferenceFailed(first)),
            },
            other => other,
        }
    }

    fn is_available(&self) -> bool {
        self.program.is_file()
    }

    fn name(&self) -> &str {
        match self.agent {
            RepoWikiAgent::Codex => "codex",
            _ => "claude-code",
        }
    }

    fn backend(&self) -> LlmBackend {
        LlmBackend::Api
    }
}

/// One answer from one tool.
async fn run_agent(
    agent: RepoWikiAgent,
    program: &std::path::Path,
    request: &LlmRequest,
) -> Result<String, LlmError> {
    let dir = tempfile::tempdir()
        .map_err(|e| LlmError::InferenceFailed(format!("agent temp dir: {e}")))?;
    let answer_file = dir.path().join("answer.txt");
    let prompt = match &request.system_prompt {
        Some(system) => format!("{system}\n\n{}", request.user_prompt),
        None => request.user_prompt.clone(),
    };
    let mut child = tokio::process::Command::new(program)
        .args(AgentCliProvider::args(agent, &answer_file))
        .current_dir(dir.path())
        .env("WENLAN_NO_AUTOSTART", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| LlmError::InferenceFailed(format!("{:?} spawn: {e}", agent)))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| LlmError::InferenceFailed(format!("{:?} stdin: {e}", agent)))?;
    }
    let output = tokio::time::timeout(ANSWER_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| LlmError::Timeout)?
        .map_err(|e| LlmError::InferenceFailed(format!("{:?} wait: {e}", agent)))?;
    if !output.status.success() {
        return Err(LlmError::InferenceFailed(format!(
            "{:?} exited {}: {}",
            agent,
            output.status,
            failure_text(&output.stderr, &output.stdout)
        )));
    }
    let answer = match agent {
        RepoWikiAgent::Codex => std::fs::read_to_string(&answer_file)
            .map_err(|e| LlmError::InferenceFailed(format!("codex answer: {e}")))?,
        _ => String::from_utf8_lossy(&output.stdout).into_owned(),
    };
    Ok(answer.trim().to_string())
}

/// Why a tool failed. Claude Code prints a sign-in failure on standard
/// output, not standard error, so fall back to it when standard error is
/// empty.
fn failure_text(stderr: &[u8], stdout: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let text = if stderr.trim().is_empty() {
        String::from_utf8_lossy(stdout)
    } else {
        stderr
    };
    text.trim().chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::failure_text;

    #[test]
    fn a_sign_in_failure_printed_on_stdout_is_reported() {
        let text = failure_text(b"", b"Invalid API key \xc2\xb7 Please run /login\n");
        assert!(text.contains("Please run /login"), "{text}");
        assert_eq!(failure_text(b"boom", b"ignored"), "boom");
    }
}
