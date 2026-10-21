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
  REGISTERED_SOURCES,
  GRAPH_ENTITIES,
  GRAPH_DETAILS,
  GRAPH_MEMORIES,
  GRAPH_PAGES,
  SOURCE_LIBRARY_FILES,
} from "../fixtures";
import { TauriMockRuntime } from "../../e2e/tauriMock/runtime";
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
import { invokeRemoteFixture } from "./remote-access";

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
      return MERGE_SOURCES[pageId] ?? SOURCES;
    }
    case "get_page_links":
      return LINKS;
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
