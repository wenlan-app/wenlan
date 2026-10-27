// SPDX-License-Identifier: AGPL-3.0-only
//
// Agent display-name resolution.
//
// Canonical technical IDs are strings like `claude-code` or `openai-mcp`.
// Users see friendly display names like `Claude Code` or `ChatGPT`.
//
// Resolution order:
//   1. User registration (`agent_connections.display_name` via listAgents).
//   2. Built-in `KNOWN_CLIENT_NAMES` registry (mirrors origin-core's `KNOWN_CLIENTS`).
//   3. The canonical ID prettified into a readable title (word-split on
//      `-`/`_`, title-case each word, acronym map for mcp/cli/api/ai/vscode).
//
// **Keep in sync with `crates/origin-core/src/db.rs::KNOWN_CLIENTS`.**

import type { AgentConnection } from "./tauri";

interface ClientNames {
  /** Broad product label used for provenance and connection surfaces. */
  display: string;
  /** Narrow label used when an action targets one specific executable. */
  action?: string;
}

export const KNOWN_CLIENT_NAMES: Record<string, ClientNames> = {
  "antigravity": { display: "Antigravity" },
  "chatgpt": { display: "ChatGPT" },
  "chatgpt-mcp": { display: "ChatGPT" },
  "claude": { display: "Claude" },
  "claude-ai": { display: "Claude" },
  "claude-code": { display: "Claude Code" },
  "claude-desktop": { display: "Claude Desktop" },
  "codex": { display: "ChatGPT / Codex", action: "Codex" },
  "continue": { display: "Continue" },
  "copilot-cli": { display: "GitHub Copilot CLI" },
  "cursor": { display: "Cursor" },
  "gemini-cli": { display: "Gemini CLI" },
  "kimi-code": { display: "Kimi Code" },
  "kimi-desktop": { display: "Kimi" },
  "kiro": { display: "Kiro" },
  "logseq": { display: "Logseq" },
  "muse-code": { display: "Muse Code" },
  "obsidian": { display: "Obsidian" },
  "openai-mcp": { display: "ChatGPT" },
  "opencode": { display: "OpenCode" },
  "raycast": { display: "Raycast" },
  "vscode": { display: "VS Code" },
  "windsurf": { display: "Windsurf" },
  "wenlan": { display: "Wenlan" },
  "wenlan-app": { display: "Wenlan" },
  "zed": { display: "Zed" },
};

/** Compatibility view for callers that need the canonical display map. */
export const KNOWN_CLIENT_DISPLAY_NAMES: Record<string, string> = Object.fromEntries(
  Object.entries(KNOWN_CLIENT_NAMES).map(([id, names]) => [id, names.display]),
);

/** Word-level overrides applied after title-casing an unknown slug's parts. */
const SLUG_WORD_OVERRIDES: Record<string, string> = {
  mcp: "MCP",
  cli: "CLI",
  api: "API",
  ai: "AI",
  vscode: "VS Code",
};

/**
 * Turn an unrecognized canonical ID into a readable title. Fabricates
 * nothing — splits on `-`/`_`, title-cases each word, and swaps known
 * acronyms/product names in. `codex-mcp-client` → `Codex MCP Client`.
 */
function prettifySlug(slug: string): string {
  return slug
    .split(/[-_]+/)
    .filter(Boolean)
    .map((word) => {
      const lower = word.toLowerCase();
      const override = SLUG_WORD_OVERRIDES[lower];
      if (override) return override;
      return lower.charAt(0).toUpperCase() + lower.slice(1);
    })
    .join(" ");
}

/**
 * Resolve a canonical agent ID to a friendly display name.
 *
 * @param canonicalId The raw value from `x-agent-name` / `activity.agent_name` /
 *                    `agent_connections.name`. Lowercased hyphen-case.
 * @param connections Optional list of registered agents, used to honor user
 *                    overrides (a user-set `display_name` always wins).
 * @returns The best available display label. Never throws. Never returns empty
 *          for a non-empty input — worst case, returns the canonical ID itself.
 */
export function resolveAgentDisplayName(
  canonicalId: string,
  connections?: AgentConnection[],
): string {
  const trimmed = canonicalId.trim();
  const normalized = trimmed.toLowerCase();
  if (!normalized) return "";

  // 1. User override via registration.
  if (connections) {
    const match = connections.find((c) => c.name.trim().toLowerCase() === normalized);
    if (match?.display_name) {
      return match.display_name;
    }
  }

  // 2. Built-in registry.
  const known = KNOWN_CLIENT_NAMES[normalized];
  if (known) return known.display;

  // Some older call sites can already hold a human-readable display value.
  // Preserve that explicit casing; only canonical slug IDs need prettifying.
  if (/\s/.test(trimmed)) return trimmed;

  // 3. Prettify the slug so an unknown integration never leaks a raw ID into
  //    visible copy.
  return prettifySlug(normalized);
}

/**
 * Resolve the shorter product name used when an action targets one concrete
 * tool rather than attributing work to the broader product family.
 */
export function resolveAgentActionName(
  canonicalId: string,
  connections?: AgentConnection[],
): string {
  const normalized = canonicalId.trim().toLowerCase();
  if (!normalized) return "";
  return KNOWN_CLIENT_NAMES[normalized]?.action
    ?? resolveAgentDisplayName(canonicalId, connections);
}

// ── Tool families ──────────────────────────────────────────────────────
// One physical tool (Codex, Claude Code, …) can register several agent
// identities — `codex`, `codex-mcp-client`, `codex-ulw-loop` are all Codex.
// The roster coalesces identities into one row per *family*; these two
// functions decide which family a client_type / agent belongs to.

/**
 * Map an MCP client's `client_type` (the wizard/ClientSetupList vocabulary)
 * to its tool family. Returns `""` for a client_type outside the known set,
 * so callers can treat "known family" as a truthy check.
 */
export function clientTypeFamily(clientType: string): string {
  switch (clientType) {
    case "claude_code":
      return "claude-code";
    case "codex_cli":
      return "codex";
    case "claude_desktop":
      return "claude-desktop";
    case "cursor":
      return "cursor";
    case "gemini_cli":
      return "gemini-cli";
    case "muse_code":
      return "muse-code";
    case "antigravity":
      return "antigravity";
    case "copilot_cli":
      return "copilot-cli";
    case "vscode":
      return "vscode";
    case "windsurf":
      return "windsurf";
    case "kiro":
      return "kiro";
    case "opencode":
      return "opencode";
    case "kimi_code":
      return "kimi-code";
    case "kimi_desktop":
      return "kimi-desktop";
    default:
      return "";
  }
}

/** `openai-mcp` is ChatGPT's registered canonical ID — fold it into the
 *  `chatgpt` family so the two never render as separate rows. */
function aliasFamily(key: string): string {
  return key === "openai-mcp" ? "chatgpt" : key;
}

/**
 * Resolve the tool family for a registered agent. Belt-and-suspenders,
 * because `agent_type` only sometimes carries the client_type vocabulary
 * (the live Codex identities did not), so name-based resolution has to
 * back it up:
 *   (a) `clientTypeFamily(agent.agent_type)` when it names a known client;
 *   (b) an exact `KNOWN_CLIENT_DISPLAY_NAMES` key (aliasing openai-mcp → chatgpt);
 *   (c) the longest known-key `prefix-` match on the name
 *       (`codex-ulw-loop` → `codex`, `claude-code-x` → `claude-code`);
 *   (d) `wenlan-setup` → `wenlan-setup`; else the raw name.
 * Never returns `""` for a non-empty name.
 */
export function toolFamilyOf(agent: { name: string; agent_type: string }): string {
  // (a) agent_type shares the client_type vocabulary — when it does, trust it.
  const fromType = clientTypeFamily(agent.agent_type);
  if (fromType) return fromType;

  const name = agent.name;

  // The setup probe deliberately stays outside Wenlan's own family even
  // though `wenlan` is a known display-name key.
  if (name === "wenlan-setup") return "wenlan-setup";

  // (b) exact canonical ID in the registry.
  if (name in KNOWN_CLIENT_DISPLAY_NAMES) return aliasFamily(name);

  // (c) longest known-key prefix (with a `-` boundary, so `codex` never
  //     swallows `codexicon`).
  let best = "";
  for (const key of Object.keys(KNOWN_CLIENT_DISPLAY_NAMES)) {
    if (name.startsWith(`${key}-`) && key.length > best.length) best = key;
  }
  if (best) return aliasFamily(best);

  // (d) bare passthrough.
  return name;
}

/** Wenlan's own identities: the app, its CLI, and the setup probe. Their calls
 *  say nothing about whether the user has connected an AI tool, so no list of
 *  the user's tools shows them. Tested against a tool family. */
export const OWN_IDENTITY = /^(wenlan|origin)(-|$)|^(manual|unknown)$/;

/** Display label for a tool family: the registry name, else the same
 *  prettifier `resolveAgentDisplayName` uses for an unrecognized slug. */
export function familyDisplayName(family: string): string {
  return resolveAgentDisplayName(family);
}

// ── Trust level presentation ───────────────────────────────────────────
// Mirrors the tier gating in `crates/origin-core/src/router/classify.rs`
// (`tier_allowed`) and `crates/origin-server/src/routes.rs` (chat-context).

/** Known trust levels the backend recognizes. Keep in sync with `register_agent`. */
export type TrustLevel = "full" | "review" | "unknown";

export interface TrustLevelDescriptor {
  level: TrustLevel;
  /** CSS accent var for the badge. */
  accent: string;
}

// Label and summary copy live in i18n (`settings.agents.trust.*` /
// `settings.agents.trustSummary.*`) — this module stays presentation-only.
export const TRUST_LEVELS: Record<TrustLevel, { accent: string }> = {
  full: { accent: "var(--mem-accent-indigo)" },
  review: { accent: "var(--mem-accent-amber)" },
  unknown: { accent: "var(--mem-accent-warm)" },
};

export function describeTrustLevel(level: string): TrustLevelDescriptor {
  const known = (level in TRUST_LEVELS ? level : "unknown") as TrustLevel;
  return { level: known, accent: TRUST_LEVELS[known].accent };
}
