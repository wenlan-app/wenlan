// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Answers daemon commands while the example wiki is open. Library reads come
 * from the example's own data, reads it has nothing for come back empty, and
 * anything that would change the library is refused. Commands about the app
 * itself (profile, settings, connected tools, setup) still reach the daemon,
 * so the rest of the app keeps working around the example.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { i18n } from "../../i18n";
import { buildExampleWiki, searchExamplePages, searchExampleWiki, type ExampleLocale, type ExampleWikiData } from "./build";

/** About the app, not the library: still answered by the daemon. */
const APP_COMMANDS = new Set([
  "acknowledge_guarded_quit_request", "cancel_guarded_quit_request", "daemon_version", "daemon_version_status",
  "detect_mcp_clients_cmd", "get_agent", "get_api_key", "get_avatar_data_url", "get_background_ai_enabled",
  "get_external_llm", "get_external_llm_key_configured", "get_knowledge_path", "get_knowledge_path_info",
  "get_model_choice", "get_on_device_model", "get_profile", "get_remote_access_profile",
  "get_remote_access_status", "get_resolved_routing", "get_setup_completed", "get_startup_preflight_status",
  "get_system_info", "get_telemetry_status", "get_wenlan_mcp_entry", "is_run_at_login_enabled", "list_agents",
  "list_external_models", "list_remote_grants", "local_setup_completed", "should_show_wizard",
  "take_remote_access_notice", "take_remote_pairing_link", "warnings", "wire_state", "get_activity",
  "get_enrichment_status", "get_index_status", "open_tool_with_prompt", "save_temp_file", "quick_capture",
  "set_api_key", "set_avatar", "remove_avatar", "set_background_ai_enabled", "set_external_llm",
  "set_model_choice", "set_run_at_login", "set_setup_completed", "set_telemetry_enabled", "set_traffic_lights_visible", "set_editor_composing",
  "update_profile", "quit_wenlan_full", "restart_daemon", "start_daemon_sidecar", "install_client_plugin",
  "install_client_skills", "register_connected_agent", "update_agent", "delete_agent", "write_mcp_config",
  "remove_legacy_mcp_entry", "remove_raw_mcp_entry", "configure_remote_access", "toggle_remote_access",
  "renew_remote_access", "reconnect_remote_access", "approve_remote_pairing", "deny_remote_pairing",
  "inspect_remote_pairing", "lookup_remote_pairing", "revoke_remote_grant", "test_external_llm",
  "test_remote_mcp_connection", "download_on_device_model", "on_device_model_download_bytes",
  "record_page_editor_diagnostic", "get_truth_status", "page_review_supported",
]);

/** Library reads the example has nothing for: an empty answer of the right shape. */
const EMPTY: Record<string, unknown> = {
  list_basis_pending: [], list_code_basis_pending: [], list_note_proposals: [], list_onboarding_milestones: [],
  list_pending_imports: [], list_pending_revisions: [], list_recent_changes: [], list_unconfirmed_memories: [],
  list_pinned_memories: [], list_watch_paths: [], get_page_sessions: [], list_recent_retrievals: [],
  get_pending_contradictions: [], list_decisions_cmd: [], list_decision_domains_cmd: [], list_activities: [],
  list_agent_activity: [], list_sources: [], read_source_dir: [], get_nurture_cards_cmd: [], get_chunks: [],
  list_working_on: [],
  active_import_batches_cmd: { batches: [] },
  list_refinements: { proposals: [] },
  list_communities: { schema_version: "1", communities: [], next_cursor: null },
  list_all_tags: { tags: [], document_tags: {}, categories: [], document_categories: {} },
  list_orphan_links: { min_count: 2, orphan_labels: [] },
  list_new_material: { items: [], total: 0, kinds: {}, held: 0 },
  repair_recovery: null, get_pending_revision: null, get_repo_wiki: null,
};

const built = new Map<ExampleLocale, ExampleWikiData>();

function data(): ExampleWikiData {
  const language = i18n.language;
  const locale: ExampleLocale = language === "zh-Hant" || language === "zh-Hans" ? language : "en";
  let wiki = built.get(locale);
  if (!wiki) {
    wiki = buildExampleWiki(locale);
    built.set(locale, wiki);
  }
  return wiki;
}

export class ExampleWikiReadOnly extends Error {
  constructor(command: string) {
    super(`The example wiki can't be changed (${command}).`);
    this.name = "ExampleWikiReadOnly";
  }
}

function answer(command: string, args: Record<string, unknown> | undefined): unknown {
  const wiki = data();
  const id = typeof args?.id === "string" ? args.id : typeof args?.pageId === "string" ? args.pageId : null;
  switch (command) {
    case "list_pages":
    case "list_pages_explicit_browse":
    case "list_recent_pages": {
      const offset = typeof args?.offset === "number" ? args.offset : 0;
      const status = typeof args?.status === "string" ? args.status : "active";
      return offset > 0 || status !== "active" ? [] : wiki.pages;
    }
    case "get_page":
    case "get_page_explicit_browse":
      return wiki.pages.find((page) => page.id === id) ?? null;
    case "get_page_sources":
      return (id && wiki.pageSources.get(id)) ?? [];
    case "get_page_links":
      return (id && wiki.pageLinks.get(id)) ?? { outbound: [], inbound: [] };
    case "list_spaces":
      return wiki.spaces;
    case "get_space":
      return wiki.spaces.find((space) => space.name === args?.name) ?? null;
    case "list_registered_sources":
      return wiki.registeredSources;
    case "list_indexed_files":
      return typeof args?.space === "string" ? wiki.indexedFiles.filter((file) => file.space === args.space) : wiki.indexedFiles;
    case "list_memories_cmd":
    case "list_recent_memories":
      return wiki.memories;
    case "list_memories_by_ids": {
      const ids = Array.isArray(args?.ids) ? args.ids : [];
      return wiki.memories.filter((memory) => ids.includes(memory.source_id));
    }
    case "get_memory_detail":
      return wiki.memories.find((memory) => memory.source_id === (args?.sourceId ?? args?.source_id)) ?? null;
    case "get_knowledge_graph_cmd":
      return wiki.graph;
    case "list_entities_cmd":
    case "query_entities_cmd":
      return wiki.entities;
    case "search_entities_cmd": {
      const query = String(args?.query ?? "").toLowerCase();
      return wiki.entities.filter((entity) => entity.name.toLowerCase().includes(query));
    }
    case "get_entity_detail_cmd":
      return wiki.entityDetails.get(String(args?.entityId ?? args?.id ?? "")) ?? null;
    case "search":
      return searchExampleWiki(wiki, String(args?.query ?? ""));
    case "search_pages":
      return searchExamplePages(wiki, String(args?.query ?? ""));
    case "get_page_revisions":
      return { page_id: id ?? "", current_version: 1, user_edited: false, entries: [] };
    case "get_memory_revisions":
      return { current_source_id: String(args?.sourceId ?? ""), chain_depth: 0, entries: [] };
    case "count_knowledge_files":
      return wiki.pages.length;
    case "read_text_file": {
      const file = wiki.indexedFiles.find((entry) => entry.source_id === args?.path);
      return file?.summary ?? "";
    }
  }
  if (command in EMPTY) return EMPTY[command];
  return undefined;
}

export function exampleInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (APP_COMMANDS.has(command)) return tauriInvoke<T>(command, args);
  const value = answer(command, args);
  if (value !== undefined) return Promise.resolve(value as T);
  // Everything else would read or change the user's own library.
  if (import.meta.env.DEV) console.warn("[example-wiki] refused", command);
  showExampleWikiReadOnly();
  return Promise.reject(new ExampleWikiReadOnly(command));
}

/** One quiet line for whatever tried to change the example; the same id never stacks. */
export function showExampleWikiReadOnly() {
  toast(i18n.t("exampleWiki.readOnly"), { id: "example-wiki-read-only" });
}
