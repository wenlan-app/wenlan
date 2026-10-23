// SPDX-License-Identifier: AGPL-3.0-only
import { UnknownTauriCommandError } from "./errors";

// Mirrors `COMMUNITY_READ_SCHEMA_VERSION` in src/lib/tauri.ts; kept as a
// literal here for the same reason runtime.ts keeps PAGE_EDIT_DAEMON_FLOOR —
// the mock must not import back through the review invoke alias.
const COMMUNITY_READ_SCHEMA_VERSION = "community-read-v1";

type BaseResponseContext = {
  readonly activityRows: readonly Record<string, unknown>[];
  readonly memoryCount: number;
};

function optionalString(args: unknown, key: string): string | null {
  const value = typeof args === "object" && args !== null ? Reflect.get(args, key) : undefined;
  return typeof value === "string" ? value : null;
}

/** Set this global before the app boots to give one spec a routing answer:
 *  `page.addInitScript(() => { window.__WENLAN_MOCK_RESOLVED_ROUTING__ = {...} })`.
 *  The shape is `ResolvedRouting` from src/lib/tauri.ts. */
export const MOCK_RESOLVED_ROUTING_KEY = "__WENLAN_MOCK_RESOLVED_ROUTING__";

/** What `get_resolved_routing` answers.
 *
 *  The default is null, and null means one specific thing: a daemon old enough
 *  to have no routing endpoint. It is NOT "nothing is pinned" — that is a real
 *  answer with `mode: "unconfigured"`. The fixture claims the older daemon
 *  because that is the state with no behavior to get wrong: the launch-time pin
 *  fill reads routing on every mount of the main shell and writes nothing on
 *  null, and Home keeps its original copy.
 *
 *  A spec that needs a real routing state, such as the unpinned everyday job
 *  that makes Home offer "Choose a model", sets the override above instead of
 *  changing this default for every other spec. */
function resolvedRouting(): unknown {
  const override = Reflect.get(globalThis, MOCK_RESOLVED_ROUTING_KEY);
  return override === undefined ? null : override;
}

export function baseResponse(command: string, args: unknown, context: BaseResponseContext): unknown {
  switch (command) {
    case "should_show_wizard": case "get_clipboard_enabled": return false;
    case "get_remote_access_profile": return null;
    case "get_remote_access_status": return { status: "off" };
    case "set_traffic_lights_visible": case "set_setup_completed": return null;
    case "list_agent_activity": return context.activityRows;
    case "list_agents": return [{ id: "agent-claude-code", name: "claude-code", display_name: "Claude Code", agent_type: "claude-code", description: null, enabled: true, trust_level: "full", last_seen_at: 1_783_728_000, memory_count: context.memoryCount, created_at: 1_783_728_000, updated_at: 1_783_728_000 }];
    case "search_pages": case "search_entities_cmd": case "list_recent_retrievals": case "list_recent_changes": case "list_recent_memories": case "list_unconfirmed_memories": case "list_recent_pages": case "list_onboarding_milestones": case "list_pending_revisions": case "get_pending_contradictions": return [];
    case "get_memory_stats_cmd": return { total: context.memoryCount, new_today: 0, confirmed: context.memoryCount, domains: [], by_type: [] };
    case "get_home_stats": return { total: context.memoryCount, new_today: 0, confirmed: context.memoryCount, total_ingested: 0, active_insights: 0, distilled_today: 0, distilled_all: 0, sources_archived: 0, times_served_today: 0, words_saved_today: 0, times_served_week: 0, words_saved_week: 0, times_served_all: 0, words_saved_all: 0, corrections_active: 0, top_memories: [] };
    // The sidebar status line mounts on EVERY page, so an unregistered
    // get_activity would make the default arm throw for every spec in the
    // suite, not just the activity ones. Up to date with empty assets is
    // the quietest shape: one still line, no counts, no popover content.
    case "get_activity": return {
      state: "up_to_date",
      last_activity_at: null,
      assets: [
        { kind: "memories", state: "idle", done: 0, total: 0, blocked: 0, steps: [] },
        { kind: "entities", state: "idle", done: 0, total: 0, blocked: 0, steps: [] },
        { kind: "pages", state: "idle", done: 0, total: 0, blocked: 0, steps: [] },
      ],
      everyday: { job: "everyday", lane: "none", model: null, mode: "unconfigured", available: false },
      synthesis: { job: "synthesis", lane: "none", model: null, mode: "unconfigured", available: false },
      refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
    };
    case "get_resolved_routing": return resolvedRouting();
    // No fixture Space is bound to a repository or has code pages to offer.
    case "get_repo_wiki": return null;
    case "suggest_repo_wiki": return { candidates: [] };
    // The main shell asks on every launch whether a wenlan://pair link is waiting.
    case "take_remote_pairing_link": return null;
    // Web access stopped on its own: the fixture never stops, so no notice waits.
    case "take_remote_access_notice": return null;
    // A short code finds the one synthetic request. The fixture knows no relay,
    // so the host is the only identity it can offer, and it is not a known app.
    case "lookup_remote_pairing": return {
      pairingId: "a".repeat(64),
      clientId: "synthetic-client-for-review-0123456789",
      resource: "https://relay.wenlan.app/mcp",
      scopes: ["wenlan:query"],
      expiresAt: 1_783_728_000_000 + 600_000,
      clientName: null,
      redirectHost: "example.com",
      knownClient: false,
    };
    case "deny_remote_pairing": return null;
    case "renew_remote_access": return { status: "off" };
    case "get_profile": case "get_pending_revision": return null;
    case "get_briefing": return { content: "", new_today: 0, primary_agent: null, generated_at: 1_783_728_000, is_stale: false };
    case "get_enrichment_status": return { source_id: optionalString(args, "sourceId") ?? "", summary: "", steps: [] };
    case "get_version_chain_cmd": return [];
    case "get_memory_revisions": return { current_source_id: optionalString(args, "sourceId") ?? "", chain_depth: 0, entries: [] };
    case "list_all_tags": return { tags: [], document_tags: {}, categories: [], document_categories: {} };
    case "get_page_links": return { outbound: [], inbound: [] };
    case "get_page_sources": case "list_registered_sources": return [];
    // The fixture has no AI conversations: no note lists one, no id resolves.
    case "get_page_sessions": return { sessions: [] };
    case "get_session_detail": return null;
    case "list_communities": return { schema_version: COMMUNITY_READ_SCHEMA_VERSION, communities: [], next_cursor: null };
    case "list_community_members": return { schema_version: COMMUNITY_READ_SCHEMA_VERSION, members: [], next_cursor: null };
    case "get_page_revisions": return { page_id: optionalString(args, "pageId") ?? "", current_version: 1, user_edited: false, entries: [] };
    // Settings export: a fixture writes no folder, so it reports an empty bundle.
    case "export_pages_as_okf": return { exported: 0, skipped: 0, failed: 0 };
    default: throw new UnknownTauriCommandError(command);
  }
}
