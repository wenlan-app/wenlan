// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  clipboardWrite,
  getBackgroundAiEnabled,
  listAgents,
  openToolWithPrompt,
  type AgentConnection,
} from "../../../lib/tauri";
import { AskAiHint } from "./AskAiHint";
import { AskInsteadHint } from "./AskInsteadHint";
import { ConnectedHint } from "./ConnectedHint";

vi.mock("../../../lib/tauri", () => ({
  PROMPT_LINK_FAMILIES: ["claude-desktop", "claude-code", "codex", "cursor"],
  openToolWithPrompt: vi.fn(),
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
    vi.mocked(clipboardWrite).mockReset().mockResolvedValue(undefined);
    vi.mocked(openToolWithPrompt).mockReset().mockResolvedValue(undefined);
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

  it("says where to ask once connected, as a line with no box, and opens the tool with /wenlan typed in", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    wrap(<ConnectedHint />);
    const hint = await screen.findByTestId("ask-ai-hint-connected");
    expect(hint).toHaveTextContent("Next, start a new chat in Claude Code and type /wenlan to ask about your notes.");
    expect(hint.querySelector("code, input, textarea")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open Claude Code" }));
    await waitFor(() => expect(openToolWithPrompt).toHaveBeenCalledWith("claude-code", "/wenlan"));
    expect(clipboardWrite).toHaveBeenCalledWith("/wenlan");
    expect(await screen.findByRole("button", { name: "Opened in Claude Code. Press Enter there." })).toBeInTheDocument();
  });

  it("copies plain words for a tool without the plugin, and says so", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("gemini-cli")]);
    wrap(<ConnectedHint />);
    const hint = await screen.findByTestId("ask-ai-hint-connected");
    expect(hint).toHaveTextContent("ask about your notes in plain words");
    expect(hint).not.toHaveTextContent("/wenlan");
    fireEvent.click(screen.getByTestId("ask-ai-hint-open"));
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("Use Wenlan: what do my notes cover so far?"));
    expect(openToolWithPrompt).not.toHaveBeenCalled();
    expect(await screen.findByTestId("ask-ai-hint-open")).toHaveTextContent("Copied.");
  });

  it("falls back to the clipboard when the tool cannot be opened", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(openToolWithPrompt).mockRejectedValue(new Error("not installed"));
    wrap(<ConnectedHint />);
    await screen.findByTestId("ask-ai-hint-connected");
    fireEvent.click(screen.getByRole("button", { name: "Open Codex" }));
    expect(await screen.findByRole("button", { name: "Copied. Paste it into Codex." })).toBeInTheDocument();
    expect(clipboardWrite).toHaveBeenCalledWith("$wenlan");
  });

  it("points a blank new page at the AI tool, and only while it is blank", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    const { rerender } = wrap(<AskInsteadHint blank />);
    expect(await screen.findByTestId("ask-instead-hint")).toHaveTextContent("Want to ask a question? Type $wenlan in Codex.");
    rerender(
      <QueryClientProvider client={new QueryClient()}>
        <AskInsteadHint blank={false} />
      </QueryClientProvider>,
    );
    expect(screen.queryByTestId("ask-instead-hint")).toBeNull();
  });
});
