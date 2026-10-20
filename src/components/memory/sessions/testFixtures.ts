// SPDX-License-Identifier: AGPL-3.0-only
import type { AgentSession, SessionDetail } from "../../../lib/tauri";

/** Oct 20 of this year at hh:mm, local time, in Unix seconds. */
export function oct20(hour: number, minute: number): number {
  return new Date(new Date().getFullYear(), 9, 20, hour, minute).getTime() / 1000;
}

export const claudeSession: AgentSession = {
  id: "ses_claude",
  tool: "claude-code",
  external_id: "00893aaf-19fa-41d2-8238-13269b9b3ca0",
  repo_root: "/work/wenlan",
  repo_branch: "main",
  repo_commit: "be0283ac71013796e7674387f78b45b82b01d54e",
  transcript_path: "/home/u/.claude/projects/wenlan/00893aaf.jsonl",
  space: "Work",
  first_write_at: oct20(9, 12),
  last_write_at: oct20(10, 40),
};

export function detailFor(session: AgentSession, over: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session,
    memories: [],
    pages: [],
    handoffs: [],
    transcript_available: true,
    ...over,
  };
}
