// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { clipboardWrite, listAgents, type AgentConnection } from "../../../lib/tauri";
import { AskAiDialog } from "./AskAiDialog";
import { defaultAskAiTool } from "./askAiCommands";

vi.mock("../../../lib/tauri", () => ({
  clipboardWrite: vi.fn(),
  listAgents: vi.fn(),
}));

function agent(name: string, lastSeen: number | null): AgentConnection {
  return {
    id: `agent-${name}`,
    name,
    display_name: null,
    agent_type: name,
    description: null,
    enabled: true,
    trust_level: "full",
    last_seen_at: lastSeen,
    memory_count: 0,
    created_at: 1,
    updated_at: 1,
  };
}

function renderDialog(props: Partial<Parameters<typeof AskAiDialog>[0]> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onClose = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <AskAiDialog onClose={onClose} {...props} />
    </QueryClientProvider>,
  );
  return { onClose };
}

function command(task: string): string {
  return within(screen.getByTestId(`ask-ai-task-${task}`)).getByRole("code").textContent ?? "";
}

describe("defaultAskAiTool", () => {
  it("opens on the tool used most recently and ignores Wenlan's own identities", () => {
    expect(defaultAskAiTool([agent("claude-code", 10), agent("codex", 20), agent("wenlan-cli", 99)]))
      .toEqual({ tool: "codex", connected: true });
    expect(defaultAskAiTool([agent("cursor", 5)])).toEqual({ tool: "other", connected: true });
    expect(defaultAskAiTool([agent("wenlan-setup", 5)])).toEqual({ tool: "claude", connected: false });
  });
});

describe("AskAiDialog", () => {
  beforeEach(async () => {
    vi.mocked(listAgents).mockReset();
    vi.mocked(clipboardWrite).mockReset();
    await i18n.changeLanguage("en");
  });

  it("writes each task the way the connected tool takes it", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex", 1)]);
    renderDialog();

    await waitFor(() => expect(screen.getByTestId("ask-ai-tab-codex")).toHaveAttribute("aria-selected", "true"));
    expect(command("steep")).toBe("$wenlan steep");
    expect(command("wiki")).toBe("$wenlan wiki");
    expect(command("brief")).toBe("$brief");
    expect(command("recall")).toBe("$recall how we chose the database");

    fireEvent.click(screen.getByTestId("ask-ai-tab-claude"));
    expect(command("steep")).toBe("/wenlan steep");
    expect(command("capture")).toBe("/capture we switched to SQLite");
  });

  it("asks other tools in a sentence and leaves out what only the plugin can do", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("cursor", 1)]);
    renderDialog();

    await waitFor(() => expect(screen.getByTestId("ask-ai-tab-other")).toHaveAttribute("aria-selected", "true"));
    expect(command("steep")).toBe("Use Wenlan to write new pages and update stale ones");
    expect(screen.queryByTestId("ask-ai-task-wiki")).toBeNull();
  });

  it("copies the command shown and offers to connect only when no tool is connected", async () => {
    vi.mocked(listAgents).mockResolvedValue([]);
    vi.mocked(clipboardWrite).mockResolvedValue(undefined);
    const onConnectTool = vi.fn();
    renderDialog({ onConnectTool });

    await waitFor(() => expect(screen.getByTestId("ask-ai-connect")).toBeInTheDocument());
    fireEvent.click(within(screen.getByTestId("ask-ai-task-steep")).getByRole("button"));
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("/wenlan steep"));
    fireEvent.click(screen.getByRole("button", { name: "Connect a tool" }));
    expect(onConnectTool).toHaveBeenCalled();
  });

  it("closes on Escape", async () => {
    vi.mocked(listAgents).mockResolvedValue([]);
    const { onClose } = renderDialog();
    fireEvent.keyDown(screen.getByTestId("ask-ai-dialog"), { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});
