// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  getBackgroundAiEnabled,
  listAgents,
  type AgentConnection,
} from "../../../lib/tauri";
import { AskAiHint } from "./AskAiHint";
import { ConnectedHint } from "./ConnectedHint";

vi.mock("../../../lib/tauri", () => ({
  clipboardWrite: vi.fn(),
  getBackgroundAiEnabled: vi.fn(),
  listAgents: vi.fn(),
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

function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{children}</QueryClientProvider>);
}

describe("ask-AI hints", () => {
  beforeEach(async () => {
    localStorage.clear();
    vi.mocked(listAgents).mockReset();
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
    await waitFor(() => expect(screen.getByTestId("steep-hint")).toHaveTextContent("Use Wenlan to write new pages and update stale ones"));
    expect(screen.queryByTestId("wiki-hint")).toBeNull();
  });

  it("opens the full list from the connected hint", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    wrap(<ConnectedHint />);
    await screen.findByTestId("ask-ai-hint-connected");
    fireEvent.click(screen.getByRole("button", { name: "See all" }));
    expect(screen.getByTestId("ask-ai-dialog")).toBeInTheDocument();
  });
});
