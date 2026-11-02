// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { PageHistoryItem } from "../../../lib/tauri";
import { HistoryChangeDialog, historyTexts } from "./HistoryChangeDialog";
import PageInfo from "./PageInfo";

const NOW = Math.floor(Date.now() / 1000);

const history: PageHistoryItem[] = [
  { kind: "ai_edit", at: NOW - 10, version: 3, edited_by: "agent_refresh", agent: "codex", text: "Tram 28 runs late. Book Sintra early." },
  {
    kind: "source_update",
    at: NOW - 20,
    version: 2,
    source: { page_id: "page-src", title: "Lisbon trams", version: 5, relied_on_version: 4, pending: true, text: "Tram 28 runs until 11 pm." },
  },
  { kind: "person_edit", at: NOW - 30, version: 2, edited_by: "manual_edit", text: "Tram 28 runs late." },
  {
    kind: "source_update",
    at: NOW - 40,
    version: 1,
    source: { page_id: "page-src", title: "Lisbon trams", version: 4, relied_on_version: 4, pending: false, text: "Tram 28 runs until 10 pm." },
  },
];

const getPageRevisions = vi.fn(async () => ({
  page_id: "page-1",
  current_version: 3,
  user_edited: true,
  entries: [],
  history,
}));

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getPageRevisions: (...args: unknown[]) => getPageRevisions(...(args as [])),
}));

function renderInfo() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PageInfo
        embedded
        sourceCount={0}
        sources={[]}
        inbound={[]}
        revisions={[]}
        history={history.map(({ text, source, ...rest }) => ({ ...rest, source: source ? { ...source, text: null } : null }))}
        citations={undefined}
        citationState="none"
        onMemoryClick={vi.fn()}
        currentPageId="page-1"
      />
    </QueryClientProvider>,
  );
  return userEvent.setup();
}

describe("page history", () => {
  it("UX-HIST-2: an edit compares with the edit before it, a source with its own earlier version", () => {
    expect(historyTexts(history, history[0])).toEqual({ before: "Tram 28 runs late.", after: "Tram 28 runs late. Book Sintra early." });
    expect(historyTexts(history, history[1])).toEqual({ before: "Tram 28 runs until 10 pm.", after: "Tram 28 runs until 11 pm." });
    expect(historyTexts(history, history[2])).toEqual({ before: null, after: "Tram 28 runs late." });
  });

  it("UX-HIST-1: a source's new version is in the same list as the edits", () => {
    renderInfo();
    const list = screen.getByRole("list", { name: "Revision history" });
    const rows = within(list).getAllByRole("button");
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining("v3"),
      expect.stringContaining("“Lisbon trams” has a new version"),
      expect.stringContaining("v2"),
    ]);
    expect(rows[1].textContent).toContain("Not checked against this page yet");
  });

  it("UX-HIST-2: opening an entry shows what that change did to the text", async () => {
    const user = renderInfo();
    await user.click(screen.getByRole("button", { name: /See what changed: .*Lisbon trams/ }));
    const dialog = await screen.findByRole("dialog");
    const text = await within(dialog).findByTestId("history-change-text");
    expect(text.querySelector("del")?.textContent).toContain("10");
    expect(text.querySelector("ins")?.textContent).toContain("11");
    expect(getPageRevisions).toHaveBeenCalledWith("page-1", true);
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("UX-HIST-2: the first edit compares with the page as it was created, which is listed as its birth", async () => {
    const birth: PageHistoryItem = { kind: "person_edit", at: NOW - 60, version: 1, edited_by: "person_create", text: "Tram 28." };
    const firstEdit: PageHistoryItem = { kind: "ai_edit", at: NOW - 50, version: 2, edited_by: "distill", text: "Tram 28 runs late." };
    const withBirth = [firstEdit, birth];
    expect(historyTexts(withBirth, firstEdit)).toEqual({ before: "Tram 28.", after: "Tram 28 runs late." });
    expect(historyTexts(withBirth, birth)).toEqual({ before: null, after: "Tram 28." });

    getPageRevisions.mockResolvedValueOnce({ page_id: "page-1", current_version: 2, user_edited: true, entries: [], history: withBirth });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <HistoryChangeDialog pageId="page-1" item={birth} onClose={vi.fn()} />
      </QueryClientProvider>,
    );
    const dialog = await screen.findByRole("dialog", { name: "You created this page" });
    expect(await within(dialog).findByText("The page as it was first written.")).toBeTruthy();
    expect(within(dialog).getByTestId("history-change-text").textContent).toBe("Tram 28.");
  });
});
