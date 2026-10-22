// SPDX-License-Identifier: AGPL-3.0-only

/**
 * Commands exercised by the fixture-only Review product surface.
 *
 * This is an allowlist, not a claim that Review mirrors every production IPC
 * command. `review/tauri-core.ts` rejects commands outside this contract.
 */
export const REVIEW_COMMAND_CAPABILITIES = {
  navigationAndHome: [
    "should_show_wizard",
    "set_traffic_lights_visible",
    "set_setup_completed",
    "search",
    "search_pages",
    "search_entities_cmd",
    "list_spaces",
    "list_memories_cmd",
    "list_memories_by_ids",
    "get_memory_stats_cmd",
    "get_home_stats",
    "list_recent_retrievals",
    "list_recent_changes",
    "list_recent_memories",
    "list_recent_pages",
    "list_entities_cmd",
    "get_knowledge_graph_cmd",
    "get_profile",
    "list_agents",
    "list_agent_activity",
    "list_onboarding_milestones",
    "acknowledge_onboarding_milestone",
    "get_briefing",
    "get_pending_contradictions",
    "get_truth_status",
    "active_import_batches_cmd",
    // The toolbar Activity button reads it on every screen.
    "get_activity",
    "get_background_ai_enabled",
    // The main shell reads routing once per launch, to give an unpinned
    // background job a source. Shared chrome, so it belongs in the contract
    // rather than in a list of screens that are allowed to fail.
    "get_resolved_routing",
    // The main shell asks once per launch whether a wenlan://pair link
    // opened the app. Shared chrome, so it belongs in the contract.
    "take_remote_pairing_link",
    // The same shell listens for "Web access stopped on its own" and asks for
    // the one pending notice. Shared chrome, so it belongs in the contract.
    "take_remote_access_notice",
  ],
  // Web access (the relay) requests. The fixture shows no live connection, so
  // these answer from baseResponses with the quietest shape; they exist so the
  // approval dialog, a denied request and a renewed key dispatch through the
  // Review runtime instead of falling through to "Unknown Tauri command".
  remoteAccess: [
    "lookup_remote_pairing",
    "deny_remote_pairing",
    "renew_remote_access",
  ],
  backgroundAi: [
    // Fixture-only preference state; this never starts a background service.
    "set_background_ai_enabled",
    // Fixture-only too: the switch locks fixture pages in memory.
    "get_lock_my_pages",
    "set_lock_my_pages",
  ],
  wikiAndPages: [
    // Isolated in-memory folders; these never touch a native knowledge root.
    "knowledge_folders_list",
    "knowledge_folder_create",
    "page_move",
    "list_pages",
    "get_page",
    "list_pages_explicit_browse",
    "get_page_explicit_browse",
    "create_page",
    "rename_page",
    "update_page",
    "delete_page",
    "redistill_page",
    "daemon_version",
    "get_system_info",
    "record_page_editor_diagnostic",
    "review_page",
    "page_review_supported",
    "get_page_links",
    "set_page_review_required",
    "mark_page_basis",
    "unmark_page_basis",
    "confirm_page_basis",
    "get_page_basis_diff",
    "list_basis_pending",
    "list_code_basis_pending",
    "confirm_page_code_basis",
    "remove_page_code_basis",
    "get_page_code_basis_cited",
    "read_code_lines",
    "read_git_blob_lines",
    "get_page_revisions",
    "get_page_sources",
    // Fixtures have no AI conversations; both answer with the quiet shape.
    "get_page_sessions",
    "get_session_detail",
    "list_registered_sources",
  ],
  pageDrafts: [
    "create_page_draft",
    "update_page_draft",
    "publish_page_draft",
    "discard_page_draft",
  ],
  pageMap: [
    "get_page_map",
    "improve_page_map",
    "create_page_map_node",
    "patch_page_map_node",
    "delete_page_map_node",
    "put_page_map_layout",
  ],
  sourceAnnotations: [
    "get_source_annotation",
    "update_source_annotation",
  ],
  refinement: [
    "distill_review",
    "list_new_material",
    "list_refinements",
    "accept_refinement",
    "reject_refinement",
    "list_pending_revisions",
    "list_unconfirmed_memories",
    "confirm_memory",
    "delete_memory",
    "get_memory_detail",
    "get_pending_revision",
    "get_enrichment_status",
    "get_version_chain_cmd",
    "get_memory_revisions",
    "repair_recovery",
    "repair_resume_runtime",
    "repair_operation_status",
    "repair_cancel",
    "repair_prepare_operation",
    "repair_prepare_operation_status",
    "repair_prepare_operation_cancel",
  ],
  memoryActions: [
    "update_memory_cmd",
    "reclassify_memory_cmd",
    "set_stability_cmd",
    "delete_file_chunks",
    "pin_memory",
    "unpin_memory",
  ],
  spaces: [
    "get_space",
    "create_space",
    "update_space",
    "delete_space",
    "confirm_space",
    "toggle_space_starred",
    "reorder_space",
  ],
  entityDetail: [
    "get_entity_detail_cmd",
    "add_observation_cmd",
    "update_observation_cmd",
    "delete_observation_cmd",
    "confirm_observation_cmd",
    "confirm_entity_cmd",
    "change_entity_type_cmd",
    "delete_entity_cmd",
  ],
  entitiesView: [
    "query_entities_cmd",
    "archive_entities_cmd",
    "restore_entities_cmd",
  ],
  cartography: [
    "list_communities",
    "list_community_members",
  ],
  existingUtilityReads: [
    "get_clipboard_enabled",
    "list_all_tags",
    "list_indexed_files",
  ],
  // Settings, General: the OKF bundle export. The fixture answers with an
  // empty bundle; a real export needs the daemon to write a folder.
  pageExport: [
    "export_pages_as_okf",
  ],
} as const;

export type ReviewCommandArea = keyof typeof REVIEW_COMMAND_CAPABILITIES;
export type ReviewCommand =
  (typeof REVIEW_COMMAND_CAPABILITIES)[ReviewCommandArea][number];

const REVIEW_COMMAND_ALLOWLIST: ReadonlySet<string> = new Set(
  Object.values(REVIEW_COMMAND_CAPABILITIES).flat(),
);

export function isReviewCommand(command: string): command is ReviewCommand {
  return REVIEW_COMMAND_ALLOWLIST.has(command);
}
