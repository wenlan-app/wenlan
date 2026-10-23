// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import AgentsSection from "./AgentsSection";
import { detectMcpClients, writeMcpConfig, type McpClient, type RemoteAccessStatus } from "../../../../lib/tauri";

// Unlike AgentsSection.test.tsx, this file does NOT stub out
// RemoteAccessPanel/ClientSetupList — it renders the real composition so the
// no-auth warning consolidation (redesign spec §6/§8) is proven against the
// actual composed Settings screen, not a component that merely intends to be
// the sole renderer.
vi.mock("../../../../lib/tauri", () => ({
  listAgents: vi.fn().mockResolvedValue([]),
  updateAgent: vi.fn().mockResolvedValue(null),
  deleteAgent: vi.fn().mockResolvedValue(null),
  detectMcpClients: vi.fn().mockResolvedValue([]),
  writeMcpConfig: vi.fn().mockResolvedValue([]),
  installClientPlugin: vi.fn().mockResolvedValue(undefined),
  installClientSkills: vi.fn().mockResolvedValue("/home/u/.config/opencode/skills"),
  removeRawMcpEntry: vi.fn().mockResolvedValue(undefined),
  clipboardWrite: vi.fn().mockResolvedValue(undefined),
  getRemoteAccessProfile: vi.fn().mockResolvedValue(null),
  configureRemoteAccess: vi.fn(),
  listSpaces: vi.fn().mockResolvedValue([{ id: "review", name: "review" }]),
  inspectRemotePairing: vi.fn(),
  approveRemotePairing: vi.fn(),
  listRemoteGrants: vi.fn().mockResolvedValue({ items: [], cursor: null }),
  revokeRemoteGrant: vi.fn(),
  getRemoteAccessStatus: vi.fn().mockResolvedValue({
    status: "connected",
    tunnel_url: "https://x.trycloudflare.com",
    relay_url: null,
  } satisfies RemoteAccessStatus),
  getWireState: vi.fn().mockResolvedValue({
    daemon: { base_url: "", reachable: true, version: null, error: null },
    mcp_binary: { command: "", args: [], undetermined: [], candidates: [] },
    clients: [],
  }),
  testRemoteMcpConnection: vi.fn().mockResolvedValue({ ok: true }),
  toggleRemoteAccess: vi.fn().mockResolvedValue(undefined),
}));

function renderAgentsSection() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <AgentsSection />
    </QueryClientProvider>,
  );
  return queryClient;
}

describe("AgentsSection composed - remote consent boundary", () => {
  it("renders one relay disclosure and no legacy no-auth claim", async () => {
    renderAgentsSection();

    // Remote Access section has loaded (status: connected) — the warning is
    // unconditional once RemoteAccessPanel has rendered.
    await screen.findByText(/reach it through wenlan-relay/);

    expect(screen.getAllByText(/reach it through wenlan-relay/)).toHaveLength(1);
    expect(screen.getAllByRole("heading", { name: "Web access" })).toHaveLength(1);
    expect(screen.queryByText(/no authentication for Claude\.ai and ChatGPT/)).not.toBeInTheDocument();
  });
});

describe("AgentsSection composed - a tool added from the list", () => {
  it("shows it once, in the list with its switch, not again as pending above", async () => {
    const yes = { kind: "yes" } as const;
    const no = { kind: "no" } as const;
    const opencode = (configured: boolean): McpClient => ({
      name: "OpenCode",
      client_type: "opencode",
      config_path: "/home/u/.config/opencode/opencode.json",
      detected: yes,
      already_configured: configured ? yes : no,
      has_raw_entry: configured ? yes : no,
      has_raw_duplicate: no,
      has_plugin: no,
      has_skills: configured ? yes : no,
    });
    let written = false;
    vi.mocked(detectMcpClients).mockImplementation(async () => [opencode(written)]);
    vi.mocked(writeMcpConfig).mockImplementationOnce(async () => {
      written = true;
      return [];
    });
    const queryClient = renderAgentsSection();

    await userEvent.click(await screen.findByRole("button", { name: /^Connect OpenCode/ }));

    // Both sections now read the written config.
    await waitFor(() => {
      const clients = queryClient.getQueryData<McpClient[]>(["mcp-clients"]);
      expect(clients?.[0]?.already_configured).toEqual(yes);
    });
    await screen.findByText(/Restart OpenCode/);
    expect(screen.getAllByText("OpenCode")).toHaveLength(1);
  });
});
