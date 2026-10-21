// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import type { AgentSession, SessionDetail } from "../../../lib/tauri";
import { claudeSession, detailFor, oct20 } from "./testFixtures";

const mocks = vi.hoisted(() => ({
  getSessionDetail: vi.fn(),
  clipboardWrite: vi.fn(),
}));

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getSessionDetail: mocks.getSessionDetail,
  clipboardWrite: mocks.clipboardWrite,
}));

import { SessionCard } from "./SessionCard";

function renderCard(
  detail: SessionDetail | null,
  props: Partial<React.ComponentProps<typeof SessionCard>> = {},
) {
  mocks.getSessionDetail.mockResolvedValue(detail);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onClose = vi.fn();
  const onOpenMemory = vi.fn();
  const onOpenPage = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <SessionCard
        sessionId="ses_claude"
        onClose={onClose}
        onOpenMemory={onOpenMemory}
        onOpenPage={onOpenPage}
        {...props}
      />
    </QueryClientProvider>,
  );
  return { onClose, onOpenMemory, onOpenPage };
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.clipboardWrite.mockResolvedValue(undefined);
});

describe("SessionCard", () => {
  it("shows the title, the time span and where it ran", async () => {
    renderCard(detailFor(claudeSession));
    const dialog = await screen.findByRole("dialog", { name: "Claude Code conversation" });
    expect(await within(dialog).findByText(/Oct 20 9:12 AM to 10:40 AM|Oct 20 9:12 to 10:40/)).toBeInTheDocument();
    expect(within(dialog).getByText((_, el) => el?.tagName === "P" && el.textContent === "Repository wenlan · Branch main · Commit be0283a")).toBeInTheDocument();
  });

  it("writes the same card in Traditional Chinese", async () => {
    await i18n.changeLanguage("zh-Hant");
    try {
      renderCard(detailFor(claudeSession));
      const dialog = await screen.findByRole("dialog", { name: "Claude Code 對話" });
      expect(await within(dialog).findByText(/10月20日 .*(?<!\d)9:12.* 到 .*10:40/)).toBeInTheDocument();
      expect(within(dialog).getByText((_, el) => el?.tagName === "P" && el.textContent === "儲存庫 wenlan · 分支 main · 提交 be0283a")).toBeInTheDocument();
      expect(within(dialog).getByRole("button", { name: "在 Claude Code 繼續這次對話" })).toBeInTheDocument();
    } finally {
      await i18n.changeLanguage("en");
    }
  });

  it("leaves the repository line out when there is no repository information", async () => {
    renderCard(detailFor({ ...claudeSession, repo_root: null, repo_branch: null, repo_commit: null }));
    await screen.findByRole("dialog");
    await screen.findByText(/Oct 20/);
    expect(screen.queryByText(/Repository|Branch|Commit/)).not.toBeInTheDocument();
  });

  it("shows a single time when the conversation was one moment", async () => {
    renderCard(detailFor({ ...claudeSession, last_write_at: claudeSession.first_write_at + 10 }));
    expect(await screen.findByText(/^Oct 20 9:12/)).not.toHaveTextContent(" to ");
  });

  it("lists handoff summaries and what the session left, as links", async () => {
    const { onClose, onOpenMemory, onOpenPage } = renderCard(
      detailFor(claudeSession, {
        handoffs: [
          { space: "Work", brief_version: 3, summary: "Shipped the writer. Next: the queue.", created_at: oct20(10, 30) },
        ],
        memories: [
          { source_id: "mem-1", title: "Writer is single", created_at: oct20(9, 30) },
          { source_id: "mem-2", title: "", created_at: oct20(9, 40) },
        ],
        pages: [
          { id: "page-1", title: "Daemon notes", version: 2, edited_at: oct20(10, 0) },
          { id: "page-1", title: "Daemon notes", version: 1, edited_at: oct20(9, 50) },
        ],
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("heading", { name: "Handoff summary" })).toBeInTheDocument();
    expect(within(dialog).getByText("Shipped the writer. Next: the queue.")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Left by this conversation" })).toBeInTheDocument();
    expect(within(dialog).getByRole("list", { name: "2 memories" })).toBeInTheDocument();
    // Two versions of one note are one note.
    expect(within(dialog).getByRole("list", { name: "1 page" })).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Untitled" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onOpenMemory).toHaveBeenCalledWith("mem-2");

    await userEvent.click(within(dialog).getByRole("button", { name: "Daemon notes" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-1");
  });

  it("uses the conversation title as the heading and moves the tool to the meta line", async () => {
    renderCard(detailFor({ ...claudeSession, title: "文兰 AI 会话资源" }));
    const dialog = await screen.findByRole("dialog", { name: "文兰 AI 会话资源" });
    expect(within(dialog).getByRole("heading", { level: 2, name: "文兰 AI 会话资源" })).toBeInTheDocument();
    expect(within(dialog).getByText(/^Claude Code · Oct 20 9:12/)).toBeInTheDocument();
    // The title is not repeated in the body.
    expect(within(dialog).getAllByText("文兰 AI 会话资源")).toHaveLength(1);
  });

  it("keeps the tool heading when the conversation has no title", async () => {
    renderCard(detailFor({ ...claudeSession, title: "   " }));
    expect(await screen.findByRole("dialog", { name: "Claude Code conversation" })).toBeInTheDocument();
    expect(screen.getByText(/^Oct 20 9:12/)).not.toHaveTextContent("Claude Code");
  });

  it("lists up to three lookups, newest first, with the rest counted", async () => {
    const { onClose, onOpenPage } = renderCard(
      detailFor(claudeSession, {
        reads: [
          { kind: "recall", label: "how do we sync", created_at: oct20(10, 30) },
          { kind: "brief", label: "Work", created_at: oct20(10, 20) },
          { kind: "read_page", label: "page-9", title: "Sync design", created_at: oct20(10, 10) },
          { kind: "recall", label: "older query", created_at: oct20(9, 50) },
        ],
        reads_total: 6,
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("heading", { name: "What it looked up" })).toBeInTheDocument();
    const list = within(dialog).getByRole("list", { name: "What it looked up" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    expect(within(list).getByText("Searched for “how do we sync”")).toBeInTheDocument();
    expect(within(list).getByText("Read the Brief for Work")).toBeInTheDocument();
    expect(within(dialog).queryByText(/older query/)).not.toBeInTheDocument();
    expect(within(dialog).getByText("3 more lookups")).toBeInTheDocument();

    await userEvent.click(within(list).getByRole("button", { name: "Read Sync design" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onOpenPage).toHaveBeenCalledWith("page-9");
  });

  it("says nothing more when every lookup is shown", async () => {
    renderCard(
      detailFor(claudeSession, {
        reads: [{ kind: "recall", label: "one", created_at: oct20(10, 30) }],
        reads_total: 1,
      }),
    );
    await screen.findByText("Searched for “one”");
    expect(screen.queryByText(/more lookup/)).not.toBeInTheDocument();
  });

  it("lists notes that later used the memories it left", async () => {
    const { onOpenPage } = renderCard(
      detailFor(claudeSession, {
        cited_by: [
          { id: "page-7", title: "Sync overview", cited_at: oct20(11, 0) },
          { id: "page-8", title: "", cited_at: oct20(11, 5) },
        ],
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("heading", { name: "Pages that later used these memories" })).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Untitled" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-8");
    await userEvent.click(within(dialog).getByRole("button", { name: "Sync overview" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-7");
  });

  it("orders the sections: handoff, lookups, left behind, later used, continue", async () => {
    renderCard(
      detailFor(claudeSession, {
        handoffs: [{ space: "Work", brief_version: 1, summary: "Done.", created_at: oct20(10, 30) }],
        reads: [{ kind: "recall", label: "q", created_at: oct20(10, 0) }],
        reads_total: 1,
        memories: [{ source_id: "m", title: "Mem", created_at: oct20(9, 30) }],
        cited_by: [{ id: "p", title: "Later note", cited_at: oct20(11, 0) }],
      }),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByRole("heading", { name: "Pages that later used these memories" });
    const order = within(dialog)
      .getAllByRole("heading", { level: 3 })
      .map((heading) => heading.textContent);
    expect(order).toEqual([
      "Handoff summary",
      "What it looked up",
      "Left by this conversation",
      "Pages that later used these memories",
    ]);
    const heading = within(dialog).getByRole("heading", { name: "Pages that later used these memories" });
    const resume = within(dialog).getByRole("button", { name: "Continue in Claude Code" });
    expect(heading.compareDocumentPosition(resume) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("writes the new sections in Traditional Chinese without dashes", async () => {
    await i18n.changeLanguage("zh-Hant");
    try {
      renderCard(
        detailFor({ ...claudeSession, title: "文蘭對話" }, {
          reads: [
            { kind: "recall", label: "同步", created_at: oct20(10, 30) },
            { kind: "brief", label: "Work", created_at: oct20(10, 20) },
            { kind: "read_page", label: "p", title: "同步設計", created_at: oct20(10, 10) },
            { kind: "recall", label: "舊的", created_at: oct20(9, 50) },
          ],
          reads_total: 5,
          cited_by: [{ id: "p2", title: "後續筆記", cited_at: oct20(11, 0) }],
        }),
      );
      const dialog = await screen.findByRole("dialog", { name: "文蘭對話" });
      expect(await within(dialog).findByRole("heading", { name: "它查了什麼" })).toBeInTheDocument();
      expect(within(dialog).getByText("搜尋了「同步」")).toBeInTheDocument();
      expect(within(dialog).getByText("還有 2 次")).toBeInTheDocument();
      expect(within(dialog).getByRole("heading", { name: "後來用到這些記憶的筆記" })).toBeInTheDocument();
      expect(dialog.textContent).not.toMatch(/[—–]/);
    } finally {
      await i18n.changeLanguage("en");
    }
  });

  it("omits the handoff and left-behind blocks when there is nothing to list", async () => {
    renderCard(detailFor(claudeSession));
    await screen.findByRole("dialog");
    await screen.findByText(/Oct 20/);
    expect(screen.queryByRole("heading", { name: "Handoff summary" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Left by this conversation" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "What it looked up" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Pages that later used these memories" })).not.toBeInTheDocument();
  });

  it("caps a long list and shows the rest on request", async () => {
    const memories = Array.from({ length: 11 }, (_, index) => ({
      source_id: `mem-${index}`,
      title: `Memory ${index}`,
      created_at: oct20(9, index),
    }));
    renderCard(detailFor(claudeSession, { memories }));
    expect(await screen.findAllByRole("button", { name: /^Memory \d+$/ })).toHaveLength(8);
    await userEvent.click(screen.getByRole("button", { name: "Show all 11" }));
    expect(screen.getAllByRole("button", { name: /^Memory \d+$/ })).toHaveLength(11);
    await userEvent.click(screen.getByRole("button", { name: "Show fewer" }));
    expect(screen.getAllByRole("button", { name: /^Memory \d+$/ })).toHaveLength(8);
  });

  it("never shows the transcript location", async () => {
    renderCard(detailFor(claudeSession));
    await screen.findByRole("button", { name: "Continue in Claude Code" });
    expect(document.body.textContent).not.toContain(".jsonl");
    expect(document.body.textContent).not.toContain("/home/u/.claude");
  });

  it("closes from the Close button and Escape", async () => {
    const first = renderCard(detailFor(claudeSession));
    await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(first.onClose).toHaveBeenCalledTimes(1);
    await userEvent.keyboard("{Escape}");
    expect(first.onClose).toHaveBeenCalledTimes(2);
  });

  it("says so when the conversation cannot be loaded", async () => {
    renderCard(null, { known: claudeSession });
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load this conversation.");
    expect(screen.getByRole("dialog", { name: "Claude Code conversation" })).toBeInTheDocument();
  });
});

describe("resume action", () => {
  const withTool = (tool: string, over: Partial<AgentSession> = {}): AgentSession => ({
    ...claudeSession,
    tool,
    external_id: "thread-42",
    ...over,
  });

  it("copies the Claude Code resume command and says it did", async () => {
    renderCard(detailFor(claudeSession));
    await userEvent.click(await screen.findByRole("button", { name: "Continue in Claude Code" }));
    expect(mocks.clipboardWrite).toHaveBeenCalledWith(`claude --resume ${claudeSession.external_id}`);
    expect(await screen.findByRole("status")).toHaveTextContent("Command copied. Paste it into your terminal.");
  });

  it("copies the Codex resume command", async () => {
    renderCard(detailFor(withTool("codex")));
    await userEvent.click(await screen.findByRole("button", { name: "Continue in Codex" }));
    expect(mocks.clipboardWrite).toHaveBeenCalledWith("codex resume thread-42");
  });

  it("reports a clipboard that refuses instead of claiming success", async () => {
    mocks.clipboardWrite.mockRejectedValue(new Error("no plugin"));
    const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
    });
    try {
      renderCard(detailFor(claudeSession));
      await userEvent.click(await screen.findByRole("button", { name: "Continue in Claude Code" }));
      expect(await screen.findByRole("status")).toHaveTextContent("Could not copy the command.");
    } finally {
      if (original) Object.defineProperty(navigator, "clipboard", original);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it.each(["opencode", "gemini-cli", "chatgpt", "claude-ai", "future-agent"])(
    "offers no button for %s",
    async (tool) => {
      renderCard(detailFor(withTool(tool)));
      await screen.findByRole("heading", { name: /conversation$/ });
      await screen.findByText(/Oct 20/);
      expect(screen.queryByRole("button", { name: /^Continue in/ })).not.toBeInTheDocument();
      expect(screen.queryByText(/cleared by/)).not.toBeInTheDocument();
    },
  );

  it("shows a muted line instead of the button when the tool cleared the transcript", async () => {
    renderCard(detailFor(claudeSession, { transcript_available: false }));
    expect(await screen.findByText("The original conversation was cleared by Claude Code")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Continue in/ })).not.toBeInTheDocument();
  });

  it("names the tool in the cleared line, including tools without a resume command", async () => {
    renderCard(detailFor(withTool("opencode"), { transcript_available: false }));
    expect(await screen.findByText("The original conversation was cleared by OpenCode")).toBeInTheDocument();
  });

  it("says it in Traditional Chinese", async () => {
    await i18n.changeLanguage("zh-Hant");
    try {
      renderCard(detailFor(claudeSession, { transcript_available: false }));
      expect(await screen.findByText("原始對話已被 Claude Code 清理")).toBeInTheDocument();
    } finally {
      await i18n.changeLanguage("en");
    }
  });

  it("shows neither when no transcript was ever recorded (imports)", async () => {
    renderCard(
      detailFor(withTool("chatgpt", { transcript_path: null }), { transcript_available: false }),
    );
    await screen.findByText(/Oct 20/);
    expect(screen.queryByText(/cleared by/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Continue in/ })).not.toBeInTheDocument();
  });

  it("offers no button when the daemon predates transcript checks", async () => {
    const detail = detailFor(claudeSession);
    delete detail.transcript_available;
    renderCard(detail);
    await screen.findByText(/Oct 20/);
    expect(screen.queryByRole("button", { name: /^Continue in/ })).not.toBeInTheDocument();
  });
});
