// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import type { McpClient } from "../../lib/tauri";
import { NO, YES } from "../../test/readings";

const mocks = vi.hoisted(() => ({
  detectMcpClients: vi.fn(),
  writeMcpConfig: vi.fn(),
  installClientPlugin: vi.fn(),
  installClientSkills: vi.fn(),
  clipboardWrite: vi.fn(),
}));
vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return { ...actual, ...mocks };
});

import ClientSetupList from "./ClientSetupList";

function client(client_type: string, name: string, detected = true): McpClient {
  return {
    name,
    client_type,
    config_path: `/config/${client_type}.json`,
    detected: detected ? YES : NO,
    already_configured: NO,
    has_raw_entry: NO,
    has_raw_duplicate: NO,
    has_plugin: NO,
    has_skills: null,
    entry_health: { kind: "no_entry" },
  };
}

function renderList(props: React.ComponentProps<typeof ClientSetupList> = {}) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <ClientSetupList {...props} />
    </QueryClientProvider>,
  );
}

describe("ClientSetupList", () => {
  beforeEach(() => {
    mocks.detectMcpClients.mockResolvedValue([client("cursor", "Cursor")]);
    mocks.writeMcpConfig.mockResolvedValue([]);
    mocks.installClientPlugin.mockResolvedValue(undefined);
    mocks.installClientSkills.mockResolvedValue("/skills");
    mocks.clipboardWrite.mockResolvedValue(undefined);
  });
  afterEach(() => Object.values(mocks).forEach((mock) => mock.mockReset()));

  it("shows the concise heading action and no discovery banner or repeated skills line", async () => {
    renderList();

    expect(await screen.findByRole("heading", { name: "On this computer, not connected" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Connect all" })).toBeInTheDocument();
    expect(screen.queryByText(/Found .*AI tool/)).not.toBeInTheDocument();
    expect(screen.queryByText("Connects and adds skills")).not.toBeInTheDocument();
  });

  it("uses the shared names and only the approved clarifying subtitles", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      client("codex_cli", "Codex CLI"),
      client("kimi_desktop", "Kimi"),
      client("kimi_code", "Kimi Code"),
      client("cursor", "Cursor"),
    ]);
    renderList();

    expect(await screen.findByText("ChatGPT / Codex")).toBeInTheDocument();
    expect(screen.getByText("ChatGPT desktop and Codex CLI share one setup")).toBeInTheDocument();
    expect(screen.getByText("Kimi desktop app")).toBeInTheDocument();
    expect(screen.getByText("Coding CLI and the Kimi Code app")).toBeInTheDocument();
    expect(screen.getByText("Cursor").parentElement?.parentElement).not.toHaveTextContent("share one setup");
  });

  it("names tools that work but are not installed in one quiet line", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      client("windsurf", "Windsurf", false),
      client("kiro", "Kiro", false),
    ]);
    renderList();

    expect(await screen.findByText("Also works with Windsurf and Kiro, not installed here.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Connect Windsurf/ })).not.toBeInTheDocument();
  });

  it("lists not-installed tools with 、 in Chinese", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      client("windsurf", "Windsurf", false),
      client("kiro", "Kiro", false),
      client("kimi_code", "Kimi Code", false),
    ]);
    await i18n.changeLanguage("zh-Hant");
    try {
      renderList();
      expect(await screen.findByText("也支援 Windsurf、Kiro、Kimi Code（這台電腦沒裝）")).toBeInTheDocument();
    } finally {
      await i18n.changeLanguage("en");
    }
  });

  it("connects every safe row from the heading action", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      client("cursor", "Cursor"),
      client("claude_desktop", "Claude Desktop"),
    ]);
    renderList();

    await userEvent.click(await screen.findByRole("button", { name: "Connect all" }));
    await waitFor(() => {
      expect(mocks.writeMcpConfig).toHaveBeenNthCalledWith(1, "cursor");
      expect(mocks.writeMcpConfig).toHaveBeenNthCalledWith(2, "claude_desktop");
    });
  });

  it("reports a successful switch immediately to its parent", async () => {
    const onClientConnected = vi.fn();
    renderList({ onClientConnected });

    await userEvent.click(await screen.findByRole("button", { name: "Connect Cursor" }));
    await waitFor(() => expect(onClientConnected).toHaveBeenCalledWith("cursor"));
  });

  it("keeps raw setup errors off the row and copies them on request", async () => {
    mocks.writeMcpConfig.mockRejectedValueOnce(new Error("EACCES /private/config.json"));
    renderList();

    await userEvent.click(await screen.findByRole("button", { name: "Connect Cursor" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't add Wenlan to Cursor.");
    expect(screen.queryByText(/EACCES/)).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Copy details" }));
    expect(mocks.clipboardWrite).toHaveBeenCalledWith("EACCES /private/config.json");
  });

  it("shows loading, retryable failure, and an empty device honestly", async () => {
    mocks.detectMcpClients.mockReturnValueOnce(new Promise(() => {}));
    const loading = renderList();
    expect(screen.getByRole("status")).toHaveTextContent("Loading available tools");
    loading.unmount();

    mocks.detectMcpClients.mockRejectedValueOnce(new Error("scan failed"));
    const failed = renderList();
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load available tools");
    failed.unmount();

    mocks.detectMcpClients.mockResolvedValueOnce([]);
    renderList();
    expect(await screen.findByText("No tools are available on this device.")).toBeInTheDocument();
  });
});
