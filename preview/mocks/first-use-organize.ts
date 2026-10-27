// SPDX-License-Identifier: AGPL-3.0-only
// Browser-only states for the import finish's "Turn them into pages" card.
// `?organize=` picks one; without it nothing here answers and the harness
// behaves as before. Fixture values only: no daemon, no clipboard writes.
//
//   deeplink   On demand, Codex connected (opens with the prompt typed in)
//   kimi       On demand, Kimi Code connected (no prompt link: copy command)
//   multi      On demand, three tools connected (Codex most recent): tool picker
//   writing    Codex is writing: 3 of 8 pages landed, more held
//   done       8 pages written a few minutes ago, 12 notes still waiting
//   held       Codex took 12 notes 4 minutes ago and nothing has come of it
//   stopped    The last run let its 12 notes go without writing a page
//   none       On demand, no AI tool connected
//   automatic  Automatic, a local model writes pages
//   chats      Checklist chat import: nothing waits until the chats land
//   folder     An Obsidian vault was just added and its first read stored
//              12 notes: the folder import's finish, On demand, Codex
//              connected. `view=sources` opens the Sources library.
import type { Page, RegisteredSource } from "../../src/lib/tauri";
import { rememberAddedFolder } from "../../src/components/memory/sources/FolderImportOrganize";

const params = new URLSearchParams(location.search);
export const organizeScenario = params.get("organize");

const RUN_KEY = "askAi:writePages:run";
const now = Date.now();
const iso = (ms: number) => new Date(ms).toISOString();
const runStartedAt = organizeScenario === "done" ? now - 6 * 60_000 : now - 60_000;

const TITLES = [
  "Kitchen renovation budget",
  "Sourdough starter routine",
  "Berlin trip in May",
  "Reading list for winter",
  "Garden layout plan",
  "Running training log",
  "Home network setup",
  "Gift ideas for family",
];

function pagesFor(scenario: string | null, base: Page | undefined): Page[] {
  if (!base) return [];
  const count = scenario === "writing" ? 3 : scenario === "done" ? 8 : 0;
  return Array.from({ length: count }, (_, i) => ({
    ...base,
    id: `organize-page-${i + 1}`,
    title: TITLES[i]!,
    status: "active",
    creation_kind: "distilled",
    created_at: iso(runStartedAt + (scenario === "done" ? 30_000 + i * 15_000 : 10_000 + i * 12_000)),
    updated_at: iso(runStartedAt + 30_000 + i * 15_000),
    source_memory_ids: Array.from({ length: 2 + (i % 3) }, (_, n) => `organize-memory-${i}-${n}`),
  }));
}

function agent(name: string, displayName: string) {
  return {
    id: `agent-${name}`, name, display_name: displayName, agent_type: name, description: null, enabled: true,
    trust_level: "full", last_seen_at: Math.floor(now / 1000), memory_count: 4,
    created_at: Math.floor(now / 1000) - 3600, updated_at: Math.floor(now / 1000),
  };
}

let chatsLanded = false;

const FOLDER_PATH = "/Users/preview/Notes";
const folderSource: RegisteredSource = {
  id: "obsidian-notes", source_type: "obsidian", path: FOLDER_PATH, status: "Active",
  last_sync: Math.floor(now / 1000) - 20, file_count: 12, memory_count: 31,
};
const FOLDER_NOTES = TITLES.slice(0, 6).map((title) => `${title}.md`);

/** Seed the run the writing/done states follow, as a press of the button
 *  would have left it. */
export function seedOrganizeRun(): void {
  try {
    if (organizeScenario === "writing" || organizeScenario === "done") {
      localStorage.setItem(RUN_KEY, JSON.stringify({ startedAt: runStartedAt, tool: "Codex", handedOver: "opened" }));
    } else if (organizeScenario === "held" || organizeScenario === "stopped") {
      // The tool claimed all 12 a while ago; "stopped" then finds them back.
      const claim = { held: 12, total: 12, at: now - 4 * 60_000 };
      localStorage.setItem(RUN_KEY, JSON.stringify({ startedAt: now - 5 * 60_000, tool: "Codex", handedOver: "opened", claim }));
    } else if (organizeScenario) {
      localStorage.removeItem(RUN_KEY);
    }
    if (organizeScenario) localStorage.removeItem("askAi:writePages:stopped");
    // What the add flow leaves behind once the vault is added.
    if (organizeScenario === "folder") rememberAddedFolder(folderSource);
  } catch {
    // Storage blocked: the states without a run still render.
  }
}

/** Answers a command for the chosen state, or `undefined` to fall through. */
export function organizeInvoke(command: string, args: Record<string, unknown> | undefined, basePage: Page | undefined): { value: unknown } | undefined {
  const scenario = organizeScenario;
  if (!scenario) return undefined;
  switch (command) {
    case "get_background_ai_enabled": return { value: scenario === "automatic" };
    case "list_agents": {
      if (scenario === "none") return { value: [] };
      if (scenario === "kimi") return { value: [agent("kimi-code", "Kimi Code")] };
      if (scenario === "multi") return { value: [agent("codex", "Codex"), { ...agent("kimi-code", "Kimi Code"), last_seen_at: Math.floor(now / 1000) - 600 }, { ...agent("cursor", "Cursor"), last_seen_at: Math.floor(now / 1000) - 1200 }] };
      return { value: [agent("codex", "Codex")] };
    }
    case "list_new_material": {
      if (scenario === "chats") {
        return { value: chatsLanded ? { items: [], total: 24, kinds: { chat: 24 }, held: 0 } : { items: [], total: 0, kinds: {}, held: 0 } };
      }
      return { value: { items: [], total: 12, kinds: { note: 12 }, held: scenario === "writing" ? 5 : scenario === "held" ? 12 : 0 } };
    }
    case "list_pages":
    case "list_pages_explicit_browse": {
      const status = typeof args?.status === "string" ? args.status : undefined;
      const offset = typeof args?.offset === "number" ? args.offset : 0;
      return { value: status === "draft" || offset > 0 ? [] : pagesFor(scenario, basePage) };
    }
    case "open_tool_with_prompt": return { value: null };
    case "get_resolved_routing": {
      if (scenario !== "automatic") return undefined;
      const route = { source: "on_device", model: "qwen3-4b", mode: "pinned", pin: "on_device" };
      return { value: { everyday: route, synthesis: route, pool: { anthropic: { configured: false, everyday_model: null, synthesis_model: null }, external: null, on_device: { selected: "qwen3-4b", loaded: true } } } };
    }
    case "get_on_device_model": {
      if (scenario !== "automatic") return undefined;
      return { value: { loaded: "qwen3-4b", selected: "qwen3-4b", models: [{ id: "qwen3-4b", display_name: "Qwen3 4B", param_count: "4B", ram_required_gb: 6, file_size_gb: 2.5, cached: true }] } };
    }
    case "save_temp_file": return { value: "/tmp/wenlan-preview/chat-export.zip" };
    case "import_chat_export": {
      chatsLanded = true;
      return { value: { import_id: "preview-chat-import", vendor: "ChatGPT", conversations_total: 24, conversations_new: 24, conversations_skipped_existing: 0, memories_stored: 96 } };
    }
    case "list_pending_imports": return { value: [] };
    case "list_registered_sources": return { value: scenario === "folder" ? [folderSource] : [] };
    case "read_source_dir": {
      if (scenario !== "folder") return undefined;
      return { value: FOLDER_NOTES.map((name) => ({ name, isDirectory: false })) };
    }
    case "list_indexed_files": {
      if (scenario !== "folder") return undefined;
      return { value: FOLDER_NOTES.map((name, i) => ({
        source: "obsidian", source_id: `${folderSource.id}::${FOLDER_PATH}/${name}::0`, title: name.replace(/\.md$/, ""),
        chunk_count: 2, last_modified: Math.floor(now / 1000) - 60 * (i + 1), source_agent: "obsidian",
      })) };
    }
    case "detect_obsidian_vaults": return { value: [] };
    default: return undefined;
  }
}
