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
//   none       On demand, no AI tool connected
//   automatic  Automatic, a local model writes pages
//   chats      Checklist chat import: nothing waits until the chats land
import type { Page } from "../../src/lib/tauri";

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

/** Seed the run the writing/done states follow, as a press of the button
 *  would have left it. */
export function seedOrganizeRun(): void {
  try {
    if (organizeScenario === "writing" || organizeScenario === "done") {
      localStorage.setItem(RUN_KEY, JSON.stringify({ startedAt: runStartedAt, tool: "Codex", handedOver: "opened" }));
    } else if (organizeScenario) {
      localStorage.removeItem(RUN_KEY);
    }
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
      return { value: { items: [], total: 12, kinds: { note: 12 }, held: scenario === "writing" ? 5 : 0 } };
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
    default: return undefined;
  }
}
