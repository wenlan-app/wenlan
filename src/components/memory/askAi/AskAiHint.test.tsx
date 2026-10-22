// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  getBackgroundAiEnabled,
  listAgents,
  listRegisteredSources,
  type AgentConnection,
  type Page,
  type RegisteredSource,
} from "../../../lib/tauri";
import { AskAiHint } from "./AskAiHint";
import { ConnectedHint } from "./ConnectedHint";
import { DistillHint, distillHintSource } from "./DistillHint";

vi.mock("../../../lib/tauri", () => ({
  clipboardWrite: vi.fn(),
  getBackgroundAiEnabled: vi.fn(),
  listAgents: vi.fn(),
  listRegisteredSources: vi.fn(),
}));

function agent(name: string): AgentConnection {
  return {
    id: `agent-${name}`,
    name,
    display_name: null,
    agent_type: name,
    description: null,
    enabled: true,
    trust_level: "full",
    last_seen_at: 1,
    memory_count: 0,
    created_at: 1,
    updated_at: 1,
  };
}

function source(overrides: Partial<RegisteredSource> = {}): RegisteredSource {
  return {
    id: "src-1",
    source_type: "obsidian",
    path: "/Users/me/Notes/Vault",
    status: "active",
    last_sync: 1,
    file_count: 128,
    memory_count: 128,
    ...overrides,
  } as RegisteredSource;
}

const distilled = { creation_kind: "distilled" } as Page;
const authored = { creation_kind: "authored" } as Page;

function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{children}</QueryClientProvider>);
}

describe("distillHintSource", () => {
  it("offers distilling only On demand, for a source with memories, before any page is written from memories", () => {
    expect(distillHintSource([source()], [authored], true)?.id).toBe("src-1");
    expect(distillHintSource([source()], [authored], false)).toBeNull();
    expect(distillHintSource([source()], [authored], undefined)).toBeNull();
    expect(distillHintSource([source()], [distilled], true)).toBeNull();
    expect(distillHintSource([source({ memory_count: 0 })], [], true)).toBeNull();
  });
});

describe("ask-AI hints", () => {
  beforeEach(async () => {
    localStorage.clear();
    vi.mocked(listAgents).mockReset();
    vi.mocked(listRegisteredSources).mockReset();
    vi.mocked(getBackgroundAiEnabled).mockReset();
    await i18n.changeLanguage("en");
  });

  it("names the command in the user's own tool and hides a task that tool cannot do", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    const { unmount } = wrap(<AskAiHint task="steep" lead="Waiting." testId="hint" />);
    await waitFor(() => expect(screen.getByTestId("hint")).toHaveTextContent("$wenlan steep"));
    unmount();

    vi.mocked(listAgents).mockResolvedValue([agent("cursor")]);
    wrap(<>
      <AskAiHint task="steep" lead="Waiting." testId="steep-hint" />
      <AskAiHint task="wiki" lead="No pages." testId="wiki-hint" />
    </>);
    await waitFor(() => expect(screen.getByTestId("steep-hint")).toHaveTextContent("Use Wenlan to update stale pages"));
    expect(screen.queryByTestId("wiki-hint")).toBeNull();
  });

  it("stays closed after the user closes the distill hint", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listRegisteredSources).mockResolvedValue([source()]);
    vi.mocked(getBackgroundAiEnabled).mockResolvedValue(false);
    const { unmount } = wrap(<DistillHint pages={[]} />);
    const hint = await screen.findByTestId("ask-ai-hint-distill");
    expect(hint).toHaveTextContent("Vault is in. To turn it into pages, type /wenlan distill");
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByTestId("ask-ai-hint-distill")).toBeNull();
    unmount();

    wrap(<DistillHint pages={[]} />);
    await waitFor(() => expect(listRegisteredSources).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId("ask-ai-hint-distill")).toBeNull();
  });

  it("opens the full list from the connected hint", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    wrap(<ConnectedHint />);
    await screen.findByTestId("ask-ai-hint-connected");
    fireEvent.click(screen.getByRole("button", { name: "See all" }));
    expect(screen.getByTestId("ask-ai-dialog")).toBeInTheDocument();
  });
});
