// SPDX-License-Identifier: AGPL-3.0-only
// Browser stand-in for @tauri-apps/api/core (aliased in vite.preview.config.ts).
// Two backends: fixture data for /preview/ (flag set inline in its index.html),
// live daemon HTTP for the real app at /.
import {
  PAGES,
  PRISTINE,
  SOURCES,
  LINKS,
  REVISIONS,
  REVIEW_STATE,
  REVIEW_MEMORIES,
  REVIEW_ENTITIES,
  REVIEW_DISTILL,
  REVIEW_FAIL,
  MERGE_SOURCES,
  RECENT_CHANGES,
  MEMORY_REVISIONS,
  PAGE_SESSIONS,
  SESSION_DETAILS,
  REGISTERED_SOURCES,
  GRAPH_ENTITIES,
  GRAPH_DETAILS,
  GRAPH_MEMORIES,
  GRAPH_PAGES,
  SOURCE_LIBRARY_FILES,
  REPEAT_CITED_SOURCES,
  THING_MAP_GRAPH,
  THING_MAP_POINTS,
} from "../fixtures";
import { TauriMockRuntime } from "../../e2e/tauriMock/runtime";
import { archiveFixturePage, restoreFixturePage } from "../../e2e/tauriMock/pageRestore";
import { createReviewDecisionFixture } from "../../e2e/fixtures/reviewDecisions";
const folderRuntime = new TauriMockRuntime(createReviewDecisionFixture("wiki-folders"), [], [], { projectionPendingOnce: new URLSearchParams(window.location.search).get("pending") === "1" });
import {
  createSpaceProjectFixture,
  SPACE_PROJECT_SOURCE_FOLDER_ID,
  SPACE_PROJECT_SOURCE_FOLDER_PATH,
} from "../../e2e/fixtures/spaceProject";
const projectState = new URLSearchParams(window.location.search).get("state");
const projectRuntime = new TauriMockRuntime(createSpaceProjectFixture(projectState === "empty"), projectState === "error" ? [{ command: "list_indexed_files", message: "Fixture source read unavailable", times: 1 }] : [], [], {}, projectState === "loading" ? { list_indexed_files: 60_000 } : {});
const projectSourceFolder = {
  id: SPACE_PROJECT_SOURCE_FOLDER_ID,
  source_type: "directory",
  path: SPACE_PROJECT_SOURCE_FOLDER_PATH,
  status: "Active",
  last_sync: 1_783_728_000,
  file_count: 2,
  memory_count: 2,
};
const projectSourceFolderEntries = [
  { name: "閱讀札記.txt", isDirectory: false },
  { name: "每週摘錄.md", isDirectory: false },
];
const projectSourceText = new Map([
  ["/preview/sources/讀書會資料/閱讀札記.txt", "讀書會・閱讀札記\n\n這是虛構的文字檔範例。每次討論後，記下最有啟發的一句話，以及它讓我們想到的問題。\n\n本週摘錄：先記錄自己的理解，再回頭核對原文。"],
  ["/preview/sources/讀書會資料/每週摘錄.md", "# 每週摘錄\n\n## 本週問題\n哪一段內容改變了我們原先的想法？\n\n## 討論後記\n保留不同觀點，並連回各自引用的段落。"],
]);
import { liveInvoke } from "./live-invoke";
import { COMMUNITY_READ_SCHEMA_VERSION, type ThingNightPoints } from "../../src/lib/tauri";
import { invokeRemoteFixture } from "./remote-access";

// `&things=1&grow=1`: this viewer has seen every night point but the newest,
// so that one grows in when the map opens (UX-CHAT-5).
{
  const params = new URLSearchParams(window.location.search);
  if (params.get("things") === "1" && params.get("grow") === "1") {
    try {
      const newest = "tm-n-rel-3";
      const seen = THING_MAP_POINTS.flatMap((thing) => thing.points.map((point) => `mem:${point.memory_id}`)).filter((id) => id !== `mem:${newest}`);
      window.localStorage.setItem("atlas.night.seen", JSON.stringify(seen));
    } catch {
      // No storage: nothing grows, the map is the same.
    }
  }
}

const HOME_NOW = Math.floor(Date.now() / 1000);
const HOME_NIGHT = (() => {
  const night = new Date();
  night.setHours(3, 0, 0, 0);
  return Math.floor(night.getTime() / 1000);
})();
/** Recently edited pages for `?mode=home`, dated from today; some yours, some the AI's. */
const homePage = (id: string, title: string, body: string, daysAgo: number, yours = false) => {
  const at = new Date(Date.now() - daysAgo * 86_400_000 - 3_600_000).toISOString();
  return {
    ...GRAPH_PAGES[1],
    id,
    title,
    content: `# ${title}\n\n${body}`,
    entity_id: null,
    created_at: at,
    last_compiled: at,
    last_modified: at,
    ...(yours ? { user_edited: true } : {}),
  };
};
const HOME_PAGES = [
  homePage("page-home-ai-memory", "AI memory systems compared: Mem0, Letta and Wenlan", "How each one stores, finds and forgets what an AI tool learned.", 0),
  homePage("page-home-mcp", "Wenlan MCP setup", "Remote MCP needs its own settings: field defaults, the origin URL, and what happens while the daemon is off.", 0, true),
  homePage("page-home-release-notes", "0.21 release notes", "Home shows only what you are working on; overnight steeping; Codex handoff.", 1),
  homePage("page-home-design-notes", "Home design notes", "One Home button; whole cards, nothing scrolls sideways.", 1, true),
  homePage("page-home-positioning", "Wenlan positioning", "Who it is for and the one sentence that says it.", 2, true),
  homePage("page-home-remote-office", "Remote Office Setup", "Notes on the remote-office arrangement.", 3, true),
  homePage("page-home-overview", "Wenlan Project Overview", "Local-first personal memory layer: daemon, desktop shell, and knowledge graph.", 8),
];

const HOME_THINGS = [
  { id: "thg_home_1", title: "Home page: what you're working on and its layout", title_by: "ai" as const, status_line: "Cards now show only your own work; checking the layout before it ships.", spaces: ["wenlan"], last_activity_at: HOME_NOW - 600, quiet: false, dreamed_at: HOME_NIGHT },
  { id: "thg_home_2", title: "Live self-testing of the new map", title_by: "ai" as const, status_line: "Night points now join their topics; next is replaying a night.", spaces: ["wenlan"], last_activity_at: HOME_NOW - 3_600, quiet: false, dreamed_at: HOME_NIGHT },
  { id: "thg_home_3", title: "Local model list", title_by: "ai" as const, status_line: "Picks a model by memory size; Ollama and LM Studio are listed.", spaces: ["wenlan"], last_activity_at: HOME_NOW - 86_400, quiet: false },
  { id: "thg_home_4", title: "Launch post", title_by: "ai" as const, status_line: "Draft is written; waiting on the final screenshots.", spaces: ["wenlan-site"], last_activity_at: HOME_NOW - 2 * 86_400, quiet: false },
  { id: "thg_home_5", title: "0.21 release", title_by: "ai" as const, status_line: "Two fixes left before tagging; waiting for CI to finish.", spaces: ["wenlan"], last_activity_at: HOME_NOW - 3 * 86_400, quiet: false },
  { id: "thg_home_6", title: "Reading group notes", title_by: "ai" as const, status_line: "Chapter four summarized; next week's question is still open.", spaces: ["reading"], last_activity_at: HOME_NOW - 4 * 86_400, quiet: false },
  { id: "thg_home_7", title: "New Mac setup", title_by: "ai" as const, status_line: "Apps moved over; backups still need a schedule.", spaces: [], last_activity_at: HOME_NOW - 5 * 86_400, quiet: false },
];

let graphObsSeq = 0;
let fixtureBackgroundAiEnabled = false;
type PreviewPageUpdateRequest = {
  readonly id: string;
  readonly content: string;
  readonly expectedVersion: number;
};
const PAGE_UPDATE_RECEIPTS = new Map<
  string,
  { readonly input: PreviewPageUpdateRequest; readonly outcome: { outcome: "saved" } }
>();

export async function invoke(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  // `pages=0` previews a new library: no pages and no milestone reached yet.
  if ((cmd === "list_pages" || cmd === "list_pages_explicit_browse" || cmd === "list_onboarding_milestones") && new URLSearchParams(location.search).get("pages") === "0") {
    return [];
  }
  // Native window composition state has no daemon or browser-fixture side effect.
  if (cmd === "set_editor_composing") return null;
  if (!(window as { __PREVIEW_FIXTURES__?: boolean }).__PREVIEW_FIXTURES__) {
    return liveInvoke(cmd, args);
  }
  if (cmd === "rename_page") {
    throw new Error("Renaming is unavailable in this fixture");
  }
  if (new URLSearchParams(window.location.search).get("mode") === "spaces") {
    if (cmd === "list_registered_sources") return projectState === "empty" ? [] : [projectSourceFolder];
    if (cmd === "read_source_dir") {
      return String(args?.path ?? "") === projectSourceFolder.path && projectState !== "empty" ? projectSourceFolderEntries : [];
    }
    if (cmd === "read_text_file") {
      return projectSourceText.get(String(args?.path ?? "")) ?? "";
    }
    if (cmd === "open_file") return null;
    return projectRuntime.invoke(cmd, args);
  }
  if (new URLSearchParams(window.location.search).get("mode") === "sources") {
    if (cmd === "list_registered_sources") return [];
    if (cmd === "list_indexed_files") return SOURCE_LIBRARY_FILES;
  }
  if (new URLSearchParams(window.location.search).get("mode") === "folders") return folderRuntime.invoke(cmd, args);
  // Fixture previews must never toggle the maintainer's background service.
  if (cmd === "get_background_ai_enabled") return fixtureBackgroundAiEnabled;
  if (cmd === "set_background_ai_enabled") {
    if (typeof args?.enabled !== "boolean") throw new Error("Invalid background AI setting");
    fixtureBackgroundAiEnabled = args.enabled;
    return;
  }
  if (new URLSearchParams(window.location.search).get("mode") === "remote-access") {
    return invokeRemoteFixture(cmd, args);
  }
  // `?mode=home`: what you're working on, from fixtures, no daemon
  // (UX-HOME-1, UX-HOME-6). One title is long enough to need two lines, and
  // one thing was steeped overnight under a long name.
  if (new URLSearchParams(window.location.search).get("mode") === "home") {
    if (cmd === "list_things") return HOME_THINGS;
    if (cmd === "list_pages_explicit_browse") return args?.status === "active" && !args?.offset ? HOME_PAGES : [];
    if (cmd === "get_thing") {
      const thing = HOME_THINGS.find((item) => item.id === args?.id) ?? HOME_THINGS[0];
      const base = { ...thing, sessions: [{ id: `ses_${thing.id}`, tool: "claude-code", title: thing.title, last_write_at: thing.last_activity_at }], pages: [], memories: [] };
      // The first thing was steeped last night: three points settled, one
      // still open, which the large card writes out.
      if (thing.id !== "thg_home_1") return base;
      const point = (id: string, title: string, ago: number) => ({ source_id: id, title, memory_type: "decision", space: "wenlan", created_at: HOME_NOW - ago });
      return {
        ...base,
        memories: [
          point("mem_home_a", "Home shows only things from your own chats", 30_000),
          point("mem_home_b", "One Home button, in the sidebar", 29_000),
          point("mem_home_c", "Cards are whole; nothing scrolls sideways", 28_000),
          point("mem_home_d", "Should the Space Brief and handoff merge into things?", 27_000),
        ],
        dream: { at: HOME_NIGHT, decided: 3, open: 1, fresh: [] },
        open_points: ["mem_home_d"],
      };
    }
  }
  // `?mode=atlas&things=1`: the big map with things being worked on, from
  // fixtures, no daemon (UX-CHAT-5). See THING_MAP_GRAPH.
  if (new URLSearchParams(window.location.search).get("things") === "1") {
    if (cmd === "get_knowledge_graph_cmd") return THING_MAP_GRAPH;
    if (cmd === "list_thing_night_points") {
      // `&quiet=1`: the same things after they have gone quiet, the first with
      // only its points that sit by a topic.
      const quiet = new URLSearchParams(window.location.search).get("quiet") === "1";
      // `&night=1`: last night wrote and connected some of these points, so
      // the map has its morning line and Replay (UX-GRAPH-5, UX-GRAPH-6).
      if (new URLSearchParams(window.location.search).get("night") === "1") {
        const nights: Record<string, ThingNightPoints["last_night"]> = {
          "tm-site": { at: HOME_NIGHT, written: ["tm-n-site-3", "tm-n-site-4", "tm-n-site-5"], linked: [{ memory_id: "tm-n-site-2", entity_id: "tm-e15" }, { memory_id: "tm-n-site-4", entity_id: "tm-e15" }] },
          "tm-rel": { at: HOME_NIGHT, written: ["tm-n-rel-2", "tm-n-rel-3"], linked: [{ memory_id: "tm-n-rel-2", entity_id: "tm-e16" }] },
        };
        return THING_MAP_POINTS.map((thing) => (nights[thing.thing_id] ? { ...thing, last_night: nights[thing.thing_id] } : thing));
      }
      if (!quiet) return THING_MAP_POINTS;
      const linked = new Set(THING_MAP_GRAPH.memory_links.map((link) => link.memory_id));
      return THING_MAP_POINTS.map((thing, i) => ({
        ...thing,
        quiet: true,
        points: i === 0 ? thing.points.filter((point) => linked.has(point.memory_id)) : thing.points,
      }));
    }
    // No regions yet: an honest empty read, not an error.
    if (cmd === "list_communities") return { schema_version: COMMUNITY_READ_SCHEMA_VERSION, scope: { kind: "space", name: args?.space ?? null }, communities: [], next_cursor: null };
    if (cmd === "list_community_members") return { schema_version: COMMUNITY_READ_SCHEMA_VERSION, scope: { kind: "space", name: args?.space ?? null }, members: [], next_cursor: null };
  }
  switch (cmd) {
    case "knowledge_folders_list":
      throw new Error("Folder browsing is unavailable in this fixture");
    case "quick_capture":
      return 1;
    case "daemon_version":
      return "0.14.1";
    case "record_page_editor_diagnostic":
      return null;
    case "get_page":
    case "get_page_explicit_browse":
      return PAGES[args?.id as string] ?? null;
    case "get_page_sources": {
      const pageId = args?.pageId as string;
      // Merge-dossier pages get their own source lists (deriveMergeLedger
      // needs keep/retire to differ); every other page falls back to the
      // shared citation-preview SOURCES array as before.
      return MERGE_SOURCES[pageId] ?? REPEAT_CITED_SOURCES[pageId] ?? SOURCES;
    }
    case "get_page_links":
      return LINKS;
    case "get_page_sentences":
      return { page_id: args?.pageId, version: 1, ai_edits: "propose", sentences: [], proposals: [] };
    case "get_page_sessions":
      return { sessions: PAGE_SESSIONS[args?.pageId as string] ?? [] };
    case "get_session_detail":
      return SESSION_DETAILS[args?.sessionId as string] ?? null;
    case "get_page_revisions":
      return { ...REVISIONS, page_id: args?.pageId };
    case "list_recent_changes": {
      const limit = (args?.limit as number) ?? RECENT_CHANGES.length;
      return RECENT_CHANGES.slice(0, limit);
    }
    case "get_memory_revisions": {
      const sourceId = args?.sourceId as string;
      return (
        MEMORY_REVISIONS[sourceId] ?? {
          current_source_id: sourceId,
          chain_depth: 1,
          entries: [],
        }
      );
    }
    case "list_registered_sources":
      return REGISTERED_SOURCES;
    case "update_page": {
      const input: PreviewPageUpdateRequest = {
        id: String(args?.id),
        content: String(args?.content),
        expectedVersion: Number(args?.expectedVersion),
      };
      const receiptKey = `${String(args?.callerId)}\u0000${String(args?.operationId)}`;
      const receipt = PAGE_UPDATE_RECEIPTS.get(receiptKey);
      if (receipt) {
        const sameRequest =
          receipt.input.id === input.id
          && receipt.input.content === input.content
          && receipt.input.expectedVersion === input.expectedVersion;
        return sameRequest
          ? structuredClone(receipt.outcome)
          : {
              outcome: "conflict",
              message:
                "operation identity was already used for different page content",
            };
      }
      const page = PAGES[input.id];
      if (!page || page.version !== input.expectedVersion) {
        return {
          outcome: "conflict",
          message: "This page changed elsewhere.",
        };
      }
      PAGES[page.id] = {
        ...page,
        content: input.content,
        citations: [],
        version: page.version + 1,
        last_modified: new Date().toISOString(),
      };
      const outcome = { outcome: "saved" } as const;
      PAGE_UPDATE_RECEIPTS.set(receiptKey, {
        input: structuredClone(input),
        outcome: structuredClone(outcome),
      });
      return outcome;
    }
    case "redistill_page": {
      const id = args?.pageId as string;
      if (PRISTINE[id]) PAGES[id] = JSON.parse(JSON.stringify(PRISTINE[id]));
      return { status: "ok", updated: true };
    }
    case "delete_page":
      delete PAGES[args?.id as string];
      return null;
    case "archive_page": {
      const id = String(args?.id ?? "");
      PAGES[id] = archiveFixturePage(PAGES[id]);
      return null;
    }
    case "restore_page": {
      const id = String(args?.id ?? "");
      const restored = restoreFixturePage(PAGES[id], Object.values(PAGES), args?.expectedVersion, args?.title);
      PAGES[id] = restored.page;
      return restored.response;
    }
    // --- review queue (DistillReviewPanel + ReviewDialog preview) ---
    case "distill_review":
      return REVIEW_DISTILL;
    case "list_pending_revisions":
      if (REVIEW_FAIL.queue) throw new Error("[preview] simulated queue failure");
      return REVIEW_STATE.revisions;
    case "accept_pending_revision":
    case "dismiss_pending_revision": {
      const id = args?.sourceId as string;
      REVIEW_STATE.revisions = REVIEW_STATE.revisions.filter(
        (r) => r.target_source_id !== id,
      );
      return { target_source_id: id, revision_source_id: `${id}-rev`, wrote: true };
    }
    case "list_refinements":
      if (REVIEW_FAIL.queue) throw new Error("[preview] simulated queue failure");
      return { proposals: REVIEW_STATE.proposals };
    case "repair_recovery":
      // Fixture mode has no pending daemon repair artifact. The product must
      // continue through its normal ready path rather than inventing one.
      return null;
    case "list_unconfirmed_memories":
      if (REVIEW_FAIL.queue) throw new Error("[preview] simulated queue failure");
      return REVIEW_STATE.captures;
    case "confirm_memory": {
      const id = args?.sourceId as string;
      REVIEW_STATE.captures = REVIEW_STATE.captures.filter((c) => c.id !== id);
      return null;
    }
    case "delete_memory": {
      // Only a review-queue fixture capture is deleted from the fixture. The
      // wizard's runtime row stores its own synthesized probe memory
      // (live-invoke.ts's PREVIEW_PROBES — store_memory no longer touches the
      // real daemon at all) and deletes it through the same live-invoke.ts
      // delete_memory handler, which reads that same map. Fixturing this
      // delete used to swallow it, so every open of the wizard preview left a
      // real "Wenlan setup check" memory behind in the developer's own
      // knowledge base. Anything this fixture doesn't own is a live id and
      // gets a live delete.
      const id = args?.sourceId as string;
      if (!REVIEW_STATE.captures.some((c) => c.id === id)) {
        return liveInvoke(cmd, args);
      }
      REVIEW_STATE.captures = REVIEW_STATE.captures.filter((c) => c.id !== id);
      return null;
    }
    case "accept_refinement":
    case "reject_refinement": {
      const id = args?.id as string;
      const action = REVIEW_STATE.proposals.find((p) => p.id === id)?.action ?? null;
      REVIEW_STATE.proposals = REVIEW_STATE.proposals.filter((p) => p.id !== id);
      return { id, action_applied: action };
    }
    case "get_memory_detail": {
      const sourceId = args?.sourceId as string;
      // The wizard's runtime row synthesizes its own probe id (see
      // PREVIEW_PROBES in live-invoke.ts) rather than this fixture's ids —
      // falling through (like delete_memory below already does) lets that
      // synthesized id resolve instead of dying on a fixture-only `null`.
      return REVIEW_MEMORIES[sourceId] ?? liveInvoke(cmd, args);
    }
    case "list_entities_cmd":
      return GRAPH_ENTITIES;
    case "list_memories_cmd":
      return GRAPH_MEMORIES;
    case "list_pages":
      return GRAPH_PAGES;
    case "get_entity_detail_cmd": {
      const entityId = args?.entityId as string;
      return GRAPH_DETAILS[entityId] ?? REVIEW_ENTITIES[entityId] ?? null;
    }
    // --- EntityDetail dossier mutations, for the "entity" preview mode ---
    // Scoped to the GRAPH_DETAILS fixture only; a REVIEW_ENTITIES id (the
    // review-queue dialogs never fire these commands) is a harmless no-op.
    case "confirm_entity_cmd": {
      const detail = GRAPH_DETAILS[args?.entityId as string];
      if (detail) detail.entity.confirmed = args?.confirmed as boolean;
      return null;
    }
    case "delete_entity_cmd": {
      const entityId = args?.entityId as string;
      delete GRAPH_DETAILS[entityId];
      const idx = GRAPH_ENTITIES.findIndex((e) => e.id === entityId);
      if (idx !== -1) GRAPH_ENTITIES.splice(idx, 1);
      return null;
    }
    case "add_observation_cmd": {
      const entityId = args?.entityId as string;
      const id = `gk-obs-${graphObsSeq++}`;
      GRAPH_DETAILS[entityId]?.observations.push({
        id,
        entity_id: entityId,
        content: args?.content as string,
        source_agent: (args?.sourceAgent as string) ?? null,
        confidence: (args?.confidence as number) ?? null,
        confirmed: false,
        created_at: Math.floor(Date.now() / 1000),
      });
      return id;
    }
    case "update_observation_cmd": {
      const observationId = args?.observationId as string;
      for (const detail of Object.values(GRAPH_DETAILS)) {
        const obs = detail.observations.find((o) => o.id === observationId);
        if (obs) obs.content = args?.content as string;
      }
      return null;
    }
    case "delete_observation_cmd": {
      const observationId = args?.observationId as string;
      for (const detail of Object.values(GRAPH_DETAILS)) {
        detail.observations = detail.observations.filter((o) => o.id !== observationId);
      }
      return null;
    }
    case "confirm_observation_cmd": {
      const observationId = args?.observationId as string;
      for (const detail of Object.values(GRAPH_DETAILS)) {
        const obs = detail.observations.find((o) => o.id === observationId);
        if (obs) obs.confirmed = args?.confirmed as boolean;
      }
      return null;
    }
    // On-open evidence for topic/suggest_entity review dialogs — a plain
    // substring match against title/content, standing in for real search.
    case "search": {
      const query = ((args?.query as string) ?? "").toLowerCase();
      const limit = (args?.limit as number) ?? 10;
      return Object.values(REVIEW_MEMORIES)
        .filter(
          (memory) =>
            memory.title.toLowerCase().includes(query) ||
            memory.content.toLowerCase().includes(query),
        )
        .slice(0, limit)
        .map((memory, index) => ({
          id: `${memory.source_id}-search`,
          content: memory.content,
          source: "memory",
          source_id: memory.source_id,
          title: memory.title,
          url: null,
          chunk_index: 0,
          last_modified: memory.last_modified,
          score: 1 - index * 0.05,
        }));
    }
    // PINNED-mode routing fixture so /preview/ settings can DOM-verify the
    // post-#357 pickers the live 0.13.2 daemon can't reach: everyday pinned to
    // on-device, synthesis pinned to Anthropic but degraded — Anthropic isn't
    // configured (see pool.anthropic.configured below), so the auto chain
    // falls back to the connected provider (→ the amber "Pinned to Anthropic
    // — using OpenAI for now" hint). LEGACY mode is what the real app shows
    // (live-invoke's get_resolved_routing 404s → null).
    case "get_resolved_routing":
      return {
        everyday: { source: "on_device", model: "qwen3-4b", mode: "pinned", pin: "on_device" },
        synthesis: { source: "external", model: "gpt-5.2", mode: "pinned_unavailable", pin: "anthropic" },
        pool: {
          anthropic: { configured: false, everyday_model: null, synthesis_model: null },
          external: { endpoint: "https://api.openai.com/v1", model: "gpt-5.2" },
          on_device: { selected: "qwen3-4b", loaded: true },
        },
      };
    case "list_agents":
      return [
        {
          id: "preview-cursor",
          name: "cursor",
          display_name: null,
          agent_type: "mcp",
          description: null,
          enabled: true,
          trust_level: "full",
          last_seen_at: Math.floor(Date.now() / 1000) - 180,
          memory_count: 14,
          created_at: Math.floor(Date.now() / 1000) - 86_400,
          updated_at: Math.floor(Date.now() / 1000) - 180,
        },
      ];
    case "set_source_pin":
      return undefined;
    default:
      // Anything this switch doesn't fixture falls through to the live map,
      // which already carries app-local defaults with the right struct shapes
      // (get_remote_access_status, get_on_device_model...).
      // Returning null here instead would white-screen the wizard and settings
      // modes, since those components read fields off the result unguarded.
      return liveInvoke(cmd, args);
  }
}

export function convertFileSrc(path: string): string {
  return path;
}

// Stubs so other @tauri-apps plugins that import from api/core still bundle.
export const isTauri = () => false;
export class Resource {
  close() {}
}
export class Channel {
  onmessage: ((msg: unknown) => void) | null = null;
}
export class PluginListener {
  unregister() {}
}
export async function addPluginListener(): Promise<PluginListener> {
  return new PluginListener();
}
export function transformCallback(): number {
  return 0;
}
