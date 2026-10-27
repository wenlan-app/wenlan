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
  removeRawMcpEntry: vi.fn(),
  writeMcpConfig: vi.fn(),
  installClientPlugin: vi.fn(),
  installClientSkills: vi.fn(),
  clipboardWrite: vi.fn(),
}));
vi.mock("../../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../../lib/tauri")>();
  return { ...actual, ...mocks };
});
vi.mock("../../RemoteAccessPanel", () => ({ RemoteAccessPanel: () => <div /> }));
vi.mock("../../../connect/ClientSetupList", () => ({ default: () => <div data-connection-section="available" /> }));

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

function client(client_type: string, name: string): McpClient {
  return {
    name,
    client_type,
    config_path: `/config/${client_type}.json`,
    detected: YES,
    already_configured: YES,
    has_raw_entry: YES,
    has_raw_duplicate: NO,
    has_plugin: NO,
    entry_health: { kind: "healthy" },
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

describe("AgentsSection connected details", () => {
  beforeEach(() => {
    mocks.listAgents.mockResolvedValue([agent("codex")]);
    mocks.detectMcpClients.mockResolvedValue([client("codex_cli", "ChatGPT / Codex")]);
    mocks.updateAgent.mockResolvedValue(null);
    mocks.deleteAgent.mockResolvedValue(null);
    mocks.removeRawMcpEntry.mockResolvedValue(undefined);
    mocks.writeMcpConfig.mockResolvedValue([]);
    mocks.installClientPlugin.mockResolvedValue(undefined);
    mocks.installClientSkills.mockResolvedValue("/skills");
    mocks.clipboardWrite.mockResolvedValue(undefined);
  });
  afterEach(() => Object.values(mocks).forEach((mock) => mock.mockReset()));

  it("coalesces identities and reveals trust, memory count, path, and friendly identity names", async () => {
    mocks.listAgents.mockResolvedValue([
      agent("codex", { memory_count: 3 }),
      agent("codex-mcp-client", { memory_count: 2 }),
    ]);
    renderSection();

    expect(await screen.findAllByText("ChatGPT / Codex")).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "ChatGPT / Codex details" }));

    expect(screen.getByRole("combobox", { name: "Trust levels: ChatGPT / Codex" })).toBeInTheDocument();
    expect(screen.getByText("5 memories")).toBeInTheDocument();
    expect(screen.getByText("/config/codex_cli.json")).toBeInTheDocument();
    expect(screen.getByText("Codex MCP Client")).toBeInTheDocument();
    expect(screen.queryByText("codex-mcp-client")).not.toBeInTheDocument();
  });

  it("applies an aggregate trust choice to every identity", async () => {
    mocks.listAgents.mockResolvedValue([
      agent("codex", { trust_level: "full" }),
      agent("codex-mcp-client", { trust_level: "review" }),
    ]);
    renderSection();
    await userEvent.click(await screen.findByRole("button", { name: "ChatGPT / Codex details" }));
    await userEvent.selectOptions(screen.getByRole("combobox"), "unknown");

    expect(mocks.updateAgent).toHaveBeenCalledWith("codex", { trustLevel: "unknown" });
    expect(mocks.updateAgent).toHaveBeenCalledWith("codex-mcp-client", { trustLevel: "unknown" });
  });

  it("requires confirmation before deleting an identity", async () => {
    renderSection();
    await userEvent.click(await screen.findByRole("button", { name: "ChatGPT / Codex details" }));
    await userEvent.click(screen.getByRole("button", { name: "Delete ChatGPT / Codex" }));
    expect(mocks.deleteAgent).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(mocks.deleteAgent.mock.calls[0][0]).toBe("codex");
  });

  it("uses the existing removal path when a connected switch is turned off", async () => {
    mocks.listAgents.mockResolvedValue([agent("cursor")]);
    mocks.detectMcpClients.mockResolvedValue([client("cursor", "Cursor")]);
    renderSection();

    await userEvent.click(await screen.findByRole("button", { name: "Disconnect Cursor" }));
    expect(mocks.removeRawMcpEntry).toHaveBeenCalledWith("cursor");
    await waitFor(() => expect(screen.queryByText("Cursor")).not.toBeInTheDocument());
  });

  it("hides Wenlan's setup probe and never exposes its internal ID", async () => {
    mocks.listAgents.mockResolvedValue([agent("wenlan-setup")]);
    mocks.detectMcpClients.mockResolvedValue([]);
    renderSection();

    expect(await screen.findByText("No connections yet.")).toBeInTheDocument();
    expect(screen.queryByText(/wenlan-setup/)).not.toBeInTheDocument();
  });

  it("uses spacing rather than horizontal dividers", async () => {
    renderSection();
    await screen.findByText("ChatGPT / Codex");
    const connected = document.querySelector('[data-connection-section="connected"]') as HTMLElement;
    expect(connected.querySelector(".divide-y")).not.toBeInTheDocument();
    expect(connected.querySelector(".border-t")).not.toBeInTheDocument();
    expect(within(connected).getByRole("heading", { name: "Connected" })).toBeInTheDocument();
  });
});
