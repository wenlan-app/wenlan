// SPDX-License-Identifier: AGPL-3.0-only
import type { AgentConnection } from "../../../lib/tauri";
import { OWN_IDENTITY, toolFamilyOf } from "../../../lib/agents";

/** The three ways a user reaches Wenlan from an AI tool. Claude Code and Codex
 *  have the plugin's commands; every other tool has only the MCP tools, so it
 *  is asked in a plain sentence. */
export type AskAiTool = "claude" | "codex" | "other";

export const ASK_AI_TOOLS: readonly AskAiTool[] = ["claude", "codex", "other"];

export type AskAiTaskId = "capture" | "recall" | "brief" | "steep" | "wiki";

export interface AskAiTask {
  readonly id: AskAiTaskId;
  readonly group: "daily" | "sometimes";
  /** What follows the tool's prefix. `null`: the task's own words come from
   *  the locale (`askAi.tasks.<id>.example`). */
  readonly command: string | null;
  /** Only the plugin's skill does this; an MCP-only tool cannot. */
  readonly skillOnly?: boolean;
}

export const ASK_AI_TASKS: readonly AskAiTask[] = [
  { id: "capture", group: "daily", command: null },
  { id: "recall", group: "daily", command: null },
  { id: "brief", group: "daily", command: "/brief" },
  { id: "steep", group: "sometimes", command: "steep" },
  { id: "wiki", group: "sometimes", command: "wiki", skillOnly: true },
];

/** Front-door prefix per tool: `/wenlan` in Claude Code, `$wenlan` in Codex. */
const FRONT_DOOR: Record<Exclude<AskAiTool, "other">, string> = {
  claude: "/wenlan",
  codex: "$wenlan",
};

/** The exact text to type for a task. `example` is the locale's own words for
 *  tasks that take some (capture, recall); `sentence` is the plain sentence
 *  for tools without commands. */
export function askAiCommand(
  task: AskAiTask,
  tool: AskAiTool,
  words: { readonly example: string; readonly sentence: string },
): string {
  if (tool === "other") return words.sentence;
  const text = task.command === null ? words.example : task.command;
  if (text.startsWith("/")) return skillCall(text, tool);
  return `${FRONT_DOOR[tool]} ${text}`;
}

/** Codex calls a plugin skill with `$`; a leading `/` there is Codex's own
 *  built-in command and would do nothing. */
function skillCall(text: string, tool: Exclude<AskAiTool, "other">): string {
  return tool === "codex" ? `$${text.slice(1)}` : text;
}

export function askAiTasksFor(tool: AskAiTool): AskAiTask[] {
  return ASK_AI_TASKS.filter((task) => tool !== "other" || !task.skillOnly);
}

function toolOfFamily(family: string): AskAiTool {
  if (family === "claude-code") return "claude";
  if (family === "codex") return "codex";
  return "other";
}

/** The tab to open on: the tool the user used most recently, else Claude
 *  Code. `connected` says whether any AI tool is connected at all. */
export function defaultAskAiTool(agents: readonly AgentConnection[]): { tool: AskAiTool; connected: boolean } {
  const tools = agents
    .filter((agent) => !OWN_IDENTITY.test(toolFamilyOf(agent)))
    .sort((a, b) => (b.last_seen_at ?? b.created_at) - (a.last_seen_at ?? a.created_at));
  if (tools.length === 0) return { tool: "claude", connected: false };
  return { tool: toolOfFamily(toolFamilyOf(tools[0])), connected: true };
}
