// SPDX-License-Identifier: AGPL-3.0-only
import {
  installClientPlugin,
  installClientSkills,
  writeMcpConfig,
  type McpClient,
  type UndeterminedInput,
} from "../../lib/tauri";
import { readingIsNo, readingIsYes } from "../../lib/reading";
import { isPluginClient } from "./pluginClients";
import { repairReasonOf } from "./clientHealth";

/** A tool that reads Wenlan's skills from its own folder (Cursor, Antigravity,
 *  GitHub Copilot, ...). The backend says so by sending `has_skills`; the
 *  plugin clients carry their skills in the plugin and get `null`. */
export function isSkillsClient(client: McpClient): boolean {
  return client.has_skills != null;
}

/** Connected (its own entry is there and would start) but the skills were
 *  measured missing. The one row state whose fix is "Add skills" alone. */
export function needsSkillsOnly(client: McpClient): boolean {
  return (
    isSkillsClient(client) &&
    readingIsNo(client.has_skills!) &&
    readingIsYes(client.has_raw_entry) &&
    repairReasonOf(client) === null
  );
}

/** The one "Set up" action, shared by the wizard and Settings so the two
 *  cannot drift:
 *  - Claude Code and Codex get their plugin and never a raw entry
 *    (`isPluginClient`), unless the raw entry is there and broken: `repair`
 *    rewrites it in place.
 *  - Every other tool gets its MCP entry, and a tool with a skills folder then
 *    gets Wenlan's skills. A tool that only lacks the skills skips the write.
 *  Resolves with what the MCP write could not determine (empty when nothing
 *  was written). */
export async function connectClient(
  client: McpClient,
  mode: "setup" | "repair",
): Promise<UndeterminedInput[]> {
  const clientType = client.client_type;
  if (mode === "setup" && isPluginClient(clientType)) {
    await installClientPlugin(clientType);
    return [];
  }
  const undetermined =
    mode === "setup" && needsSkillsOnly(client) ? [] : await writeMcpConfig(clientType);
  if (isSkillsClient(client) && !readingIsYes(client.has_skills!)) {
    await installClientSkills(clientType);
  }
  return undetermined;
}
