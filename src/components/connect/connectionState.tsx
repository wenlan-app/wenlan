// SPDX-License-Identifier: AGPL-3.0-only
//
// Shared definitions for the setup wizard and Settings > Connections.
//
//   added      Wenlan wrote the plugin or MCP entry. The tool may not even be
//              running with it yet, so this claims nothing about a link.
//   connected  The tool has since called Wenlan: an agent identity in its
//              family has a `last_seen_at` (a search counts, not only a
//              write) or a stored memory.
//
// Keeping the two words apart is the point. A written config used to render
// as "Configured" in green and the Done screen listed it under "Connected:",
// so a tool that was never restarted looked exactly like one that worked.

import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { listAgents, type AgentConnection } from "../../lib/tauri";
import { clientTypeFamily, toolFamilyOf } from "../../lib/agents";
import { StatusChip } from "../memory/settings/primitives";

export type ConnectionStatus = "added" | "connected";

/** Both Connections sections use the same quiet disclosure threshold. */
export const CONNECTION_ROW_LIMIT = 5;

/** The tool family an MCP client belongs to: the same key `toolFamilyOf` gives
 *  its agent identities, so the two sides of the comparison always agree. A
 *  client outside the known set is its own family. */
export function familyOfClient(clientType: string): string {
  return clientTypeFamily(clientType) || clientType;
}

/** The tool family of an id that is either a `client_type` (`codex_cli`) or an
 *  agent name (`codex-mcp-client`). The wizard's connected list mixes both. */
export function familyOfAnyId(id: string): string {
  return clientTypeFamily(id) || toolFamilyOf({ name: id, agent_type: "" });
}

/** The label that belongs inside a restart instruction. Most tools restart as
 *  the same product name the row displays. The shared ChatGPT / Codex config
 *  is the exception: either client can load it, so the action copy names both
 *  choices instead of reading the display-name slash aloud. */
export function restartNameForClient(
  clientTypeOrFamily: string,
  displayName: string,
  t: TFunction,
): string {
  return familyOfAnyId(clientTypeOrFamily) === "codex"
    ? t("connectMatrix.restartNameCodex")
    : displayName;
}

/** Whether an agent identity called Wenlan at or after `since` (epoch seconds).
 *
 *  Both sides are whole seconds, so a call in the same second as `since`
 *  counts (`>=`): a tool's first call landing in the second the wizard opened
 *  is a sighting, not a miss. One definition, used by every "since the wizard
 *  opened" check. */
export function heardSince(agent: Pick<AgentConnection, "last_seen_at">, since: number): boolean {
  return agent.last_seen_at != null && agent.last_seen_at >= since;
}

/** Families with at least one identity that has been heard from.
 *
 *  `since` (epoch seconds) narrows that to sightings from that moment on
 *  (`heardSince`), which is what the wizard needs: an older install's
 *  `last_seen_at` says nothing about the config it just wrote. Without
 *  `since`, a stored memory also counts, so a tool that wrote before
 *  `last_seen_at` existed is still seen. */
export function seenFamiliesOf(
  agents: readonly AgentConnection[] | undefined,
  since?: number,
): Set<string> {
  const seen = new Set<string>();
  for (const agent of agents ?? []) {
    const heard =
      since === undefined
        ? agent.last_seen_at != null || agent.memory_count > 0
        : heardSince(agent, since);
    if (heard) seen.add(toolFamilyOf(agent));
  }
  return seen;
}

/** The wizard keeps its compact chips; Settings uses sentence statuses. */
export function ConnectionChip({ status }: { status: ConnectionStatus }) {
  const { t } = useTranslation();
  return status === "connected" ? (
    <StatusChip state={{ kind: "up" }} label={t("connectMatrix.connected")} />
  ) : (
    <StatusChip state={{ kind: "idle" }} label={t("connectMatrix.added")} />
  );
}

/** Settings refreshes the roster this often while it is on screen. */
export const AGENTS_POLL_MS = 30_000;

/** The agent roster, kept live: it refetches when the window regains focus
 *  (the user comes back from restarting a tool) and on a slow interval while
 *  the window is visible. The app-wide default turns focus refetch off, so
 *  this has to opt in. */
export function useLiveAgents(intervalMs: number = AGENTS_POLL_MS, enabled: boolean = true) {
  return useQuery({
    queryKey: ["agents"],
    queryFn: listAgents,
    enabled,
    refetchOnWindowFocus: true,
    refetchInterval: intervalMs,
    refetchIntervalInBackground: false,
  });
}
