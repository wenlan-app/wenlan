// SPDX-License-Identifier: AGPL-3.0-only
import type { AgentSession } from "./tauri";
import { resolveAgentActionName, resolveAgentDisplayName } from "./agents";

/** Product name for a session tool; unknown IDs are still made readable. */
export function sessionToolLabel(tool: string): string {
  return resolveAgentDisplayName(tool);
}

/** Executable name for actions that resume a session in its source tool. */
export function sessionToolActionLabel(tool: string): string {
  return resolveAgentActionName(tool);
}

function lastSegment(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? "";
}

/** Short repository name: the folder of the repository root, else the remote's name. */
export function sessionRepoName(session: AgentSession): string | null {
  const fromRoot = session.repo_root ? lastSegment(session.repo_root) : "";
  if (fromRoot) return fromRoot;
  const remote = session.repo_remote?.trim().replace(/\/+$/, "").replace(/\.git$/, "");
  const fromRemote = remote ? (remote.split(/[\\/:]/).filter(Boolean).pop() ?? "") : "";
  return fromRemote || null;
}

/** Where the session ran, in the words a person uses: repository, else Space, else nothing. */
export function sessionPlaceName(session: AgentSession): string | null {
  return sessionRepoName(session) ?? (session.space?.trim() || null);
}

export function shortCommit(commit: string | null | undefined): string | null {
  const trimmed = commit?.trim();
  return trimmed ? trimmed.slice(0, 7) : null;
}

const SAFE_EXTERNAL_ID = /^[A-Za-z0-9._-]{1,128}$/;

/**
 * The terminal command that reopens a conversation, for the tools that have
 * one. Other tools, and ids that are not plain, get none.
 */
export function sessionResumeCommand(tool: string, externalId: string): string | null {
  if (!SAFE_EXTERNAL_ID.test(externalId)) return null;
  if (tool === "claude-code") return `claude --resume ${externalId}`;
  if (tool === "codex") return `codex resume ${externalId}`;
  return null;
}

/** "Oct 20" / "10月20日"; the year joins when it is not this year. */
export function formatSessionDay(seconds: number, locale: string, now: Date = new Date()): string {
  const date = new Date(seconds * 1000);
  if (!Number.isFinite(date.getTime())) return "";
  return date.toLocaleDateString(locale, {
    month: "short",
    day: "numeric",
    ...(date.getFullYear() === now.getFullYear() ? {} : { year: "numeric" }),
  });
}

export function formatSessionClock(seconds: number, locale: string): string {
  const date = new Date(seconds * 1000);
  if (!Number.isFinite(date.getTime())) return "";
  return date.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" });
}
