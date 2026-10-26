// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { AgentSession } from "../../../lib/tauri";
import { claudeSession, detailFor, oct20 } from "./testFixtures";

const mocks = vi.hoisted(() => ({
  getSessionDetail: vi.fn(),
  getPageSessions: vi.fn(),
  clipboardWrite: vi.fn(),
}));

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getSessionDetail: mocks.getSessionDetail,
  getPageSessions: mocks.getPageSessions,
  clipboardWrite: mocks.clipboardWrite,
}));

import {
  MemorySessionSource,
  PageSessionSources,
  SessionSourceSection,
} from "./SessionSourceSection";

const codexSession: AgentSession = {
  id: "ses_codex",
  tool: "codex",
  external_id: "019a-thread",
  space: "Research",
  first_write_at: oct20(8, 0),
  last_write_at: oct20(8, 30),
};

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.getSessionDetail.mockImplementation(async (id: string) =>
    detailFor(id === codexSession.id ? codexSession : claudeSession),
  );
});

describe("SessionSourceSection", () => {
  it("renders nothing when there are no conversations", () => {
    const { container } = renderWithClient(<SessionSourceSection wrote={[]} cited={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows tool, date and repository on one link line", () => {
    renderWithClient(<SessionSourceSection wrote={[claudeSession]} />);
    expect(screen.getByRole("heading", { name: "Source conversation" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "From Claude Code · Oct 20 · wenlan" })).toBeInTheDocument();
  });

  it("falls back to the Space name, then leaves the place out", () => {
    renderWithClient(
      <SessionSourceSection wrote={[codexSession, { ...codexSession, id: "ses_bare", space: null }]} />,
    );
    expect(screen.getByRole("button", { name: "From ChatGPT / Codex · Oct 20 · Research" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "From ChatGPT / Codex · Oct 20" })).toBeInTheDocument();
  });

  it("prettifies unknown tool IDs", () => {
    renderWithClient(<SessionSourceSection wrote={[{ ...codexSession, tool: "future-agent", space: null }]} />);
    expect(screen.getByRole("button", { name: "From Future Agent · Oct 20" })).toBeInTheDocument();
  });

  it("counts the conversations a note only cites, and expands them on click", async () => {
    renderWithClient(
      <SessionSourceSection
        wrote={[claudeSession]}
        cited={[codexSession, { ...codexSession, id: "ses_two", tool: "opencode" }]}
      />,
    );
    expect(screen.queryByRole("button", { name: /From ChatGPT \/ Codex/ })).not.toBeInTheDocument();
    const more = screen.getByRole("button", { name: "Also cites 2 conversations" });
    expect(more).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(more);
    expect(more).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "From ChatGPT / Codex · Oct 20 · Research" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "From OpenCode · Oct 20 · Research" })).toBeInTheDocument();
  });

  it("uses the singular and the cites-only wording when nothing wrote the note", () => {
    const { unmount } = renderWithClient(<SessionSourceSection wrote={[claudeSession]} cited={[codexSession]} />);
    expect(screen.getByRole("button", { name: "Also cites 1 conversation" })).toBeInTheDocument();
    unmount();
    renderWithClient(<SessionSourceSection wrote={[]} cited={[codexSession]} />);
    expect(screen.getByRole("button", { name: "Cites 1 conversation" })).toBeInTheDocument();
  });

  it("opens the conversation card from a line", async () => {
    renderWithClient(<SessionSourceSection wrote={[claudeSession]} />);
    await userEvent.click(screen.getByRole("button", { name: /From Claude Code/ }));
    expect(await screen.findByRole("dialog", { name: "Claude Code conversation" })).toBeInTheDocument();
    await waitFor(() => expect(mocks.getSessionDetail).toHaveBeenCalledWith("ses_claude"));
  });
});

describe("MemorySessionSource", () => {
  it("stays hidden for a memory without a session and asks for nothing", () => {
    const { container } = renderWithClient(<MemorySessionSource sessionId={null} />);
    expect(container).toBeEmptyDOMElement();
    expect(mocks.getSessionDetail).not.toHaveBeenCalled();
  });

  it("shows the conversation that wrote the memory", async () => {
    renderWithClient(<MemorySessionSource sessionId="ses_claude" />);
    expect(await screen.findByRole("button", { name: "From Claude Code · Oct 20 · wenlan" })).toBeInTheDocument();
    expect(mocks.getSessionDetail).toHaveBeenCalledWith("ses_claude");
  });

  it("stays hidden when the daemon cannot show the session", async () => {
    mocks.getSessionDetail.mockResolvedValue(null);
    const { container } = renderWithClient(<MemorySessionSource sessionId="ses_gone" />);
    await waitFor(() => expect(mocks.getSessionDetail).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });
});

describe("PageSessionSources", () => {
  it("lists writers as lines and folds the cited ones into a count", async () => {
    mocks.getPageSessions.mockResolvedValue([
      { session: claudeSession, via: "wrote" },
      { session: codexSession, via: "source" },
    ]);
    renderWithClient(<PageSessionSources pageId="page-1" />);
    expect(await screen.findByRole("button", { name: "From Claude Code · Oct 20 · wenlan" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Also cites 1 conversation" })).toBeInTheDocument();
    expect(mocks.getPageSessions).toHaveBeenCalledWith("page-1");
  });

  it("stays hidden for a note with no conversations or when the read fails", async () => {
    mocks.getPageSessions.mockResolvedValueOnce([]);
    const first = renderWithClient(<PageSessionSources pageId="page-1" />);
    await waitFor(() => expect(mocks.getPageSessions).toHaveBeenCalled());
    expect(first.container).toBeEmptyDOMElement();
    first.unmount();

    mocks.getPageSessions.mockRejectedValueOnce(new Error("404"));
    const second = renderWithClient(<PageSessionSources pageId="page-2" />);
    await waitFor(() => expect(mocks.getPageSessions).toHaveBeenCalledTimes(2));
    expect(second.container).toBeEmptyDOMElement();
  });
});
