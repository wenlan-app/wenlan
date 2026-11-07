// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getConversationLines, getMemorySaid, getSessionDetail, type MemorySaid, type PageSentence } from "../../../lib/tauri";
import { ThisSentence } from "./ThisSentence";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getMemorySaid: vi.fn(),
  getConversationLines: vi.fn(),
  getSessionDetail: vi.fn(),
}));

const said: MemorySaid = {
  session_id: "ses_1",
  tool: "claude-code",
  title: "Release 0.21",
  lines: [
    {
      index: 2,
      line: { speaker: "you", text: "那就一次發 0.21，等 Zotero 那幾個 PR 進來。", at: 1_793_527_260 },
      before: { speaker: "ai", text: "Pieces are safer." },
    },
  ],
};

function sentence(overrides: Partial<PageSentence> = {}): PageSentence {
  return {
    idx: 0,
    text: "We decided to ship 0.21 in one go.",
    hash: "h1",
    author: "ai",
    protected: false,
    evidence: "verified",
    citations: [{ occurrence: 1, marker: 1, source_kind: "memory", locator: "mem_a", status: "verified" }],
    ...overrides,
  } as PageSentence;
}

function renderIt(value: PageSentence) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <ThisSentence pageId="page-1" sentence={value} onFixMyself={vi.fn()} disabled={false} />
    </QueryClientProvider>,
  );
}

describe("SaidLines", () => {
  beforeEach(() => {
    vi.mocked(getMemorySaid).mockReset().mockResolvedValue(said);
    vi.mocked(getSessionDetail).mockReset().mockRejectedValue(new Error("not needed"));
    vi.mocked(getConversationLines).mockReset().mockResolvedValue({
      session_id: "ses_1",
      tool: "claude-code",
      title: "Release 0.21",
      first_index: 0,
      lines: [
        { speaker: "you", text: "Should 0.21 go out in pieces?" },
        { speaker: "ai", text: "Pieces are safer." },
        { speaker: "you", text: "那就一次發 0.21，等 Zotero 那幾個 PR 進來。" },
      ],
    });
  });

  it("UX-WHO-6: a sentence shows the line it was said in, who said it, and opens the conversation there", async () => {
    renderIt(sentence());
    expect(await screen.findByText("那就一次發 0.21，等 Zotero 那幾個 PR 進來。")).toBeTruthy();
    expect(getMemorySaid).toHaveBeenCalledWith("mem_a");
    expect(screen.getByText("In the conversation")).toBeTruthy();
    expect(screen.getByText("You")).toBeTruthy();
    // Finding the line is not checking it: nothing near it says verified.
    expect(screen.getByTestId("said-lines").textContent).not.toMatch(/verified|checked/i);

    fireEvent.click(screen.getByRole("button", { name: /那就一次發 0\.21/ }));
    const dialog = await screen.findByTestId("conversation-dialog");
    await waitFor(() => expect(dialog.querySelectorAll(".search-quote-line").length).toBe(3));
    const hit = dialog.querySelector(".search-quote-line--hit");
    expect(hit?.textContent).toContain("那就一次發 0.21");
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByTestId("conversation-dialog")).toBeNull();
  });

  it("UX-WHO-6: says so when no line says the same thing, instead of guessing", async () => {
    vi.mocked(getMemorySaid).mockResolvedValue({ ...said, lines: [] });
    renderIt(sentence());
    expect(await screen.findByText("Not found in the conversation.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Whole conversation" })).toBeTruthy();
  });

  it("UX-WHO-6: shows nothing new when the source was not a conversation", async () => {
    vi.mocked(getMemorySaid).mockRejectedValue(new Error("404"));
    renderIt(sentence());
    await waitFor(() => expect(getMemorySaid).toHaveBeenCalled());
    expect(screen.queryByTestId("said-lines")).toBeNull();
    renderIt(sentence({ hash: "h2", citations: [{ occurrence: 1, marker: 1, source_kind: "external_url", locator: "https://example.com", status: "verified" }] }));
    expect(getMemorySaid).toHaveBeenCalledTimes(1);
  });
});
