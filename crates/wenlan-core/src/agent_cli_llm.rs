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
}

impl AgentCliProvider {
    /// The first of Claude Code and Codex installed on this computer.
    pub fn find() -> Option<Self> {
        [RepoWikiAgent::Claude, RepoWikiAgent::Codex]
            .into_iter()
            .find_map(|agent| {
                crate::repo_wiki::find_agent(agent).map(|program| Self { agent, program })
            })
    }

    /// The program's arguments; the prompt goes on standard input.
    fn args(&self, answer_file: &std::path::Path) -> Vec<String> {
        match self.agent {
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
                "--allowedTools".into(),
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
        let dir = tempfile::tempdir()
            .map_err(|e| LlmError::InferenceFailed(format!("agent temp dir: {e}")))?;
        let answer_file = dir.path().join("answer.txt");
        let prompt = match &request.system_prompt {
            Some(system) => format!("{system}\n\n{}", request.user_prompt),
            None => request.user_prompt.clone(),
        };
        let mut child = tokio::process::Command::new(&self.program)
            .args(self.args(&answer_file))
            .current_dir(dir.path())
            .env("WENLAN_NO_AUTOSTART", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| LlmError::InferenceFailed(format!("{:?} spawn: {e}", self.agent)))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .await
                .map_err(|e| LlmError::InferenceFailed(format!("{:?} stdin: {e}", self.agent)))?;
        }
        let output = tokio::time::timeout(ANSWER_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| LlmError::Timeout)?
            .map_err(|e| LlmError::InferenceFailed(format!("{:?} wait: {e}", self.agent)))?;
        if !output.status.success() {
            return Err(LlmError::InferenceFailed(format!(
                "{:?} exited {}: {}",
                self.agent,
                output.status,
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(500)
                    .collect::<String>()
            )));
        }
        let answer = match self.agent {
            RepoWikiAgent::Codex => std::fs::read_to_string(&answer_file)
                .map_err(|e| LlmError::InferenceFailed(format!("codex answer: {e}")))?,
            _ => String::from_utf8_lossy(&output.stdout).into_owned(),
        };
        Ok(answer.trim().to_string())
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
