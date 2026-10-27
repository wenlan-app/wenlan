// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import {
  clientTypeFamily,
  familyDisplayName,
  resolveAgentActionName,
  resolveAgentDisplayName,
  toolFamilyOf,
} from "./agents";
import type { AgentConnection } from "./tauri";

describe("resolveAgentDisplayName", () => {
  it("uses the built-in registry for known IDs", () => {
    expect(resolveAgentDisplayName("claude-code")).toBe("Claude Code");
    expect(resolveAgentDisplayName("codex")).toBe("ChatGPT / Codex");
    expect(resolveAgentDisplayName("openai-mcp")).toBe("ChatGPT");
    expect(resolveAgentDisplayName("chatgpt-mcp")).toBe("ChatGPT");
    expect(resolveAgentDisplayName("claude-ai")).toBe("Claude");
    expect(resolveAgentDisplayName("copilot-cli")).toBe("GitHub Copilot CLI");
  });

  it("prefers a user's registered display_name over the registry", () => {
    const connections: AgentConnection[] = [
      { name: "claude-code", display_name: "My Laptop" } as AgentConnection,
    ];
    expect(resolveAgentDisplayName("claude-code", connections)).toBe("My Laptop");
  });

  it("prettifies an unrecognized slug instead of passing it through raw", () => {
    expect(resolveAgentDisplayName("codex-mcp-client")).toBe("Codex MCP Client");
    expect(resolveAgentDisplayName("My Bot")).toBe("My Bot");
  });

  it("applies the acronym map for cli/api/ai/vscode", () => {
    expect(resolveAgentDisplayName("codex-ulw-loop")).toBe("Codex Ulw Loop");
    expect(resolveAgentDisplayName("cursor-vscode")).toBe("Cursor VS Code");
    expect(resolveAgentDisplayName("some-ai-cli")).toBe("Some AI CLI");
  });

  it("handles underscores the same as hyphens", () => {
    expect(resolveAgentDisplayName("foo_bar_baz")).toBe("Foo Bar Baz");
  });

  it("never returns empty for a non-empty input", () => {
    expect(resolveAgentDisplayName("x")).toBe("X");
  });
});

describe("resolveAgentActionName", () => {
  it("keeps a narrow executable name for Codex CLI actions", () => {
    expect(resolveAgentActionName("codex")).toBe("Codex");
    expect(resolveAgentActionName("codex", [
      { name: "codex", display_name: "ChatGPT / Codex" } as AgentConnection,
    ])).toBe("Codex");
    expect(resolveAgentActionName("claude-code")).toBe("Claude Code");
    expect(resolveAgentActionName("my-bot")).toBe("My Bot");
  });
});

describe("clientTypeFamily", () => {
  it("maps each known client_type to its family", () => {
    expect(clientTypeFamily("claude_code")).toBe("claude-code");
    expect(clientTypeFamily("codex_cli")).toBe("codex");
    expect(clientTypeFamily("claude_desktop")).toBe("claude-desktop");
    expect(clientTypeFamily("cursor")).toBe("cursor");
    expect(clientTypeFamily("gemini_cli")).toBe("gemini-cli");
    expect(clientTypeFamily("muse_code")).toBe("muse-code");
  });

  it("returns empty string for an unknown client_type", () => {
    expect(clientTypeFamily("obsidian")).toBe("");
    expect(clientTypeFamily("")).toBe("");
  });
});

describe("toolFamilyOf", () => {
  it("(a) trusts agent_type when it names a known client", () => {
    expect(toolFamilyOf({ name: "whatever", agent_type: "codex_cli" })).toBe("codex");
    expect(toolFamilyOf({ name: "x", agent_type: "claude_code" })).toBe("claude-code");
  });

  it("(b) matches an exact registry key when agent_type is unhelpful", () => {
    expect(toolFamilyOf({ name: "cursor", agent_type: "" })).toBe("cursor");
    expect(toolFamilyOf({ name: "codex", agent_type: "mcp" })).toBe("codex");
  });

  it("(b) aliases openai-mcp into the chatgpt family", () => {
    expect(toolFamilyOf({ name: "openai-mcp", agent_type: "" })).toBe("chatgpt");
  });

  it("(c) coalesces suffixed identities by longest known-key prefix", () => {
    expect(toolFamilyOf({ name: "codex-ulw-loop", agent_type: "" })).toBe("codex");
    expect(toolFamilyOf({ name: "codex-mcp-client", agent_type: "" })).toBe("codex");
    // `claude-code` must win over `claude-desktop`'s shorter shared root.
    expect(toolFamilyOf({ name: "claude-code-x", agent_type: "" })).toBe("claude-code");
  });

  it("keeps Kimi (the desktop app) and Kimi Code apart", () => {
    expect(clientTypeFamily("kimi_desktop")).toBe("kimi-desktop");
    expect(clientTypeFamily("kimi_code")).toBe("kimi-code");
    expect(toolFamilyOf({ name: "kimi-desktop", agent_type: "" })).toBe("kimi-desktop");
    expect(toolFamilyOf({ name: "kimi-code", agent_type: "" })).toBe("kimi-code");
    expect(familyDisplayName("kimi-desktop")).toBe("Kimi");
    expect(familyDisplayName("kimi-code")).toBe("Kimi Code");
  });

  it("(d) keeps the wizard probe as its own family, then passes names through", () => {
    expect(toolFamilyOf({ name: "wenlan-setup", agent_type: "" })).toBe("wenlan-setup");
    expect(toolFamilyOf({ name: "my-custom-bot", agent_type: "" })).toBe("my-custom-bot");
  });
});

describe("familyDisplayName", () => {
  it("uses the registry for a known family, prettifies an unknown one", () => {
    expect(familyDisplayName("codex")).toBe("ChatGPT / Codex");
    expect(familyDisplayName("claude-code")).toBe("Claude Code");
    expect(familyDisplayName("my-custom-bot")).toBe("My Custom Bot");
  });
});
