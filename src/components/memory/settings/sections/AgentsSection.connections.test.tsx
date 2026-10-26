// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "../../../../i18n";
import type { AgentConnection, McpClient } from "../../../../lib/tauri";
import { NO, YES } from "../../../../test/readings";

const mocks = vi.hoisted(() => ({
  listAgents: vi.fn(),
  updateAgent: vi.fn(),
  deleteAgent: vi.fn(),
  detectMcpClients: vi.fn(),
  writeMcpConfig: vi.fn(),
  installClientPlugin: vi.fn(),
  installClientSkills: vi.fn(),
  removeRawMcpEntry: vi.fn(),
  clipboardWrite: vi.fn(),
}));

vi.mock("../../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../../lib/tauri")>();
  return { ...actual, ...mocks };
});
vi.mock("../../RemoteAccessPanel", () => ({ RemoteAccessPanel: () => <div data-testid="web-access" /> }));

import AgentsSection from "./AgentsSection";

function agent(name: string, overrides: Partial<AgentConnection> = {}): AgentConnection {
  return {
    id: name,
    name,
    display_name: null,
    agent_type: "mcp",
    description: null,
    enabled: true,
    trust_level: "full",
    last_seen_at: 1_700_000_000,
    memory_count: 1,
    created_at: 0,
    updated_at: 1_700_000_000,
    ...overrides,
  };
}

function client(client_type: string, name: string, configured = false): McpClient {
  return {
    name,
    client_type,
    config_path: `/config/${client_type}.json`,
    detected: YES,
    already_configured: configured ? YES : NO,
    has_raw_entry: configured ? YES : NO,
    has_raw_duplicate: NO,
    has_plugin: NO,
    has_skills: null,
    entry_health: configured ? { kind: "healthy" } : { kind: "no_entry" },
  };
}

function renderSection() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <AgentsSection />
    </QueryClientProvider>,
  );
}

function section(name: "connected" | "available") {
  return document.querySelector(`[data-connection-section="${name}"]`) as HTMLElement;
}

describe("Connections settings layout", () => {
  beforeEach(() => {
    mocks.listAgents.mockResolvedValue([]);
    mocks.updateAgent.mockResolvedValue(null);
    mocks.deleteAgent.mockResolvedValue(null);
    mocks.detectMcpClients.mockResolvedValue([]);
    mocks.writeMcpConfig.mockResolvedValue([]);
    mocks.installClientPlugin.mockResolvedValue(undefined);
    mocks.installClientSkills.mockResolvedValue("/skills");
    mocks.removeRawMcpEntry.mockResolvedValue(undefined);
    mocks.clipboardWrite.mockResolvedValue(undefined);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    Object.values(mocks).forEach((mock) => mock.mockReset());
  });

  it("renders one family row when a configured client and identity describe the same tool", async () => {
    mocks.listAgents.mockResolvedValue([agent("codex")]);
    mocks.detectMcpClients.mockResolvedValue([client("codex_cli", "ChatGPT / Codex", true)]);
    renderSection();

    await screen.findByText("ChatGPT / Codex");
    expect(screen.getAllByText("ChatGPT / Codex")).toHaveLength(1);
    expect(within(section("available")).queryByText("ChatGPT / Codex")).not.toBeInTheDocument();
  });

  it("moves a tool into Connected immediately after its switch is turned on", async () => {
    mocks.detectMcpClients.mockResolvedValue([client("cursor", "Cursor")]);
    renderSection();

    expect(await within(section("available")).findByText("Cursor")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Connect Cursor" }));

    await waitFor(() => expect(within(section("connected")).getByText("Cursor")).toBeInTheDocument());
    expect(within(section("available")).queryByText("Cursor")).not.toBeInTheDocument();
  });

  it("moves a disconnected tool back down through the existing remove path", async () => {
    mocks.listAgents.mockResolvedValue([agent("cursor")]);
    mocks.detectMcpClients.mockResolvedValue([client("cursor", "Cursor", true)]);
    renderSection();

    await userEvent.click(await screen.findByRole("button", { name: "Disconnect Cursor" }));
    expect(mocks.removeRawMcpEntry).toHaveBeenCalledWith("cursor");
    await waitFor(() => expect(within(section("available")).getByText("Cursor")).toBeInTheDocument());
    expect(within(section("connected")).queryByText("Cursor")).not.toBeInTheDocument();
  });

  it("uses the same row geometry in Connected and On this computer", async () => {
    mocks.listAgents.mockResolvedValue([agent("cursor")]);
    mocks.detectMcpClients.mockResolvedValue([
      client("cursor", "Cursor", true),
      client("claude_desktop", "Claude Desktop"),
    ]);
    renderSection();

    await screen.findByText("Claude Desktop");
    const connectedRow = section("connected").querySelector<HTMLElement>("[data-connection-row]");
    const availableRow = section("available").querySelector<HTMLElement>("[data-connection-row]");
    expect(connectedRow).toHaveClass("px-3", "py-2.5");
    expect(availableRow).toHaveClass("px-3", "py-2.5");
    expect(availableRow?.lastElementChild).toHaveClass("flex", "h-7", "items-center");
    const reservedChevronSlot = availableRow?.querySelector("[data-connection-chevron-slot]");
    expect(reservedChevronSlot).toHaveAttribute("aria-hidden", "true");
    expect(reservedChevronSlot).toHaveClass("h-7", "w-7", "shrink-0");
  });

  it("collapses both sections at five rows with one shared limit", async () => {
    mocks.listAgents.mockResolvedValue([
      agent("codex"), agent("claude-code"), agent("cursor"), agent("claude-desktop"),
      agent("antigravity"), agent("copilot-cli"),
    ]);
    mocks.detectMcpClients.mockResolvedValue([
      client("gemini_cli", "Gemini CLI"), client("muse_code", "Muse Code"),
      client("vscode", "VS Code"), client("windsurf", "Windsurf"),
      client("kiro", "Kiro"), client("opencode", "OpenCode"),
    ]);
    renderSection();

    expect(await screen.findAllByRole("button", { name: "Show 1 more" })).toHaveLength(2);
    expect(section("connected").querySelectorAll("[data-family]")).toHaveLength(5);
    expect(section("available").querySelectorAll("[data-family]")).toHaveLength(5);
  });

  it("keeps every attention row visible and sorts attention before recent use", async () => {
    const clients = [
      client("codex_cli", "ChatGPT / Codex", true),
      client("claude_code", "Claude Code", true),
      {
        ...client("opencode", "OpenCode", true),
        entry_health: { kind: "needs_repair", reason: "mcp_binary_outdated", detail: "old binary" },
      } satisfies McpClient,
    ];
    mocks.listAgents.mockResolvedValue([
      agent("cursor", { last_seen_at: 1_700_000_040 }),
      agent("gemini-cli", { last_seen_at: 1_700_000_030 }),
      agent("muse-code", { last_seen_at: 1_700_000_020 }),
      agent("vscode", { last_seen_at: 1_700_000_010 }),
    ]);
    mocks.detectMcpClients.mockResolvedValue(clients);
    renderSection();

    await screen.findByText("Needs a quick fix");
    const rows = [...section("connected").querySelectorAll<HTMLElement>("[data-family]")];
    expect(rows.slice(0, 3).map((row) => row.dataset.status)).toEqual(["repair", "restart", "restart"]);
    expect(rows.map((row) => row.dataset.family)).toEqual(expect.arrayContaining(["opencode", "codex", "claude-code"]));
  });

  it("shows every detected row on first run when nothing is connected", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      client("codex_cli", "ChatGPT / Codex"), client("claude_code", "Claude Code"),
      client("cursor", "Cursor"), client("gemini_cli", "Gemini CLI"),
      client("muse_code", "Muse Code"), client("vscode", "VS Code"),
      client("opencode", "OpenCode"),
    ]);
    renderSection();

    await screen.findByText("OpenCode");
    expect(section("available").querySelectorAll("[data-family]")).toHaveLength(7);
    expect(within(section("available")).queryByRole("button", { name: /Show .* more/ })).not.toBeInTheDocument();
  });

  it("uses one concise status form for connected, restart, and repair states", async () => {
    vi.spyOn(Date, "now").mockReturnValue(1_700_000_180 * 1000);
    mocks.listAgents.mockResolvedValue([agent("cursor", { last_seen_at: 1_700_000_000 })]);
    mocks.detectMcpClients.mockResolvedValue([
      client("codex_cli", "ChatGPT / Codex", true),
      {
        ...client("opencode", "OpenCode", true),
        entry_health: { kind: "needs_repair", reason: "mcp_binary_outdated", detail: "old binary" },
      } satisfies McpClient,
    ]);
    renderSection();

    expect(await screen.findByText("Connected · used 3m ago")).toBeInTheDocument();
    expect(screen.getByText("Restart ChatGPT or Codex to finish")).toBeInTheDocument();
    expect(screen.getByText("Needs a quick fix")).toBeInTheDocument();
    const repairButton = screen.getByRole("button", { name: "Fix" });
    expect(repairButton).toHaveClass("border", "border-[var(--mem-border)]");
    expect(repairButton.style.height).toBe("30px");
    expect(repairButton.style.paddingInline).toBe("12px");
    expect(repairButton.style.borderRadius).toBe("8px");
    expect(repairButton.style.fontSize).toBe("13px");
    expect(screen.queryByText("Added")).not.toBeInTheDocument();
  });
});
