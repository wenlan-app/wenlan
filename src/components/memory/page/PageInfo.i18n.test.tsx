// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type {
  MemoryItem,
  PageCitation,
  PageSourceWithMemory,
} from "../../../lib/tauri";
import { i18n } from "../../../i18n";
import PageInfo from "./PageInfo";

const memory = (id: string, over: Partial<MemoryItem> = {}): MemoryItem => ({
  source_id: id,
  title: `Title ${id}`,
  content: `Content of ${id}.`,
  summary: null,
  memory_type: "memory",
  domain: null,
  source_agent: "claude-code",
  confidence: null,
  confirmed: true,
  pinned: false,
  supersedes: null,
  last_modified: 1_700_000_000,
  chunk_count: 1,
  ...over,
});

const source = (id: string, over: Partial<MemoryItem> = {}): PageSourceWithMemory => ({
  source: { page_id: "page-1", memory_source_id: id, linked_at: 0 },
  memory: memory(id, over),
});

const cite = (
  occurrence: number,
  marker: number,
  locator: string,
  over: Partial<PageCitation> = {},
): PageCitation => ({
  occurrence,
  marker,
  source_kind: "memory",
  locator,
  score: 0.9,
  status: "verified",
  scope: "sentence",
  ...over,
});

function renderInfo(over: Partial<React.ComponentProps<typeof PageInfo>> = {}) {
  const onMemoryClick = vi.fn();
  const onPageClick = vi.fn();
  const utils = render(
    <PageInfo
      sourceCount={0}
      sources={[]}
      inbound={[]}
      revisions={[]}
      citations={undefined}
      citationState="none"
      onMemoryClick={onMemoryClick}
      onPageClick={onPageClick}
      {...over}
    />,
  );
  return { onMemoryClick, onPageClick, user: userEvent.setup(), ...utils };
}

afterEach(async () => {
  await i18n.changeLanguage("en");
});

describe("PageInfo i18n", () => {
  it("keeps English counters, incoming line, and diagnostics byte-identical", async () => {
    await i18n.changeLanguage("en");
    const { user } = renderInfo({
      sourceCount: 1,
      sources: [source("mem-a")],
      inbound: [{ source_page_id: "p-1", label: "Back" }],
      revisions: [
        {
          version: 2,
          at: Math.floor(Date.now() / 1000),
          edited_by: "distill",
          delta_summary: "Delta",
          incoming_source_ids: ["mem-1"],
        },
      ],
      citations: [cite(1, 1, "mem-a"), cite(2, 2, "mem-b", { status: "unverified" })],
      citationState: "cited",
    });
    expect(screen.getByText("1 backlink · 1 revision · 1 source")).toBeInTheDocument();
    await user.click(screen.getByText("Note info"));
    expect(screen.getByText("1 incoming memory")).toBeInTheDocument();
    expect(
      screen.getByText("Citations: 2 (1 unverified)"),
    ).toBeInTheDocument();
    // Known agent display names and source titles stay untouched.
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("Title mem-a")).toBeInTheDocument();
  });

  it("keeps the English unknown-agent fallback and unknown source kinds", async () => {
    await i18n.changeLanguage("en");
    const { user } = renderInfo({
      sourceCount: 2,
      sources: [
        source("mem-a", { source_agent: null, memory_type: "memory" }),
        source("mem-b", { memory_type: "pdfx" }),
      ],
    });
    await user.click(screen.getByText("Note info"));
    const rows = screen.getAllByTestId("page-info-source-row");
    expect(within(rows[0]).getByText("unknown agent")).toBeInTheDocument();
    // Arbitrary data values are never translated.
    expect(within(rows[1]).getByText("pdfx")).toBeInTheDocument();
  });

  it("localizes labels, counters, badges, and diagnostics in zh-Hans", async () => {
    await i18n.changeLanguage("zh-Hans");
    const { user } = renderInfo({
      sourceCount: 1,
      sources: [source("mem-a", { source_agent: null })],
      citations: [cite(1, 1, "mem-a", { status: "unverified" })],
      citationState: "cited",
    });
    expect(
      screen.getByText("0 条反向链接 · 0 个修订 · 1 个来源"),
    ).toBeInTheDocument();
    await user.click(screen.getByText("笔记信息"));
    const headings = screen.getAllByRole("heading", { level: 4 });
    expect(headings.map((h) => h.textContent)).toEqual(["来源"]);
    const row = screen.getByTestId("page-info-source-row");
    expect(within(row).getByText("未验证")).toBeInTheDocument();
    expect(within(row).getByText("未知代理")).toBeInTheDocument();
    expect(within(row).getByText("记忆")).toBeInTheDocument();
    // Source titles stay in their original language.
    expect(within(row).getByText("Title mem-a")).toBeInTheDocument();
    expect(screen.getByText("引用：1（1 未验证）")).toBeInTheDocument();
  });

  it("localizes stripped diagnostics and relative times in zh-Hans", async () => {
    await i18n.changeLanguage("zh-Hans");
    const dayAgo = Math.floor(Date.now() / 1000) - 86_400;
    const ids = ["mem-1", "mem-2", "mem-3", "mem-4", "mem-5", "mem-6"];
    const { user } = renderInfo({
      sourceCount: ids.length,
      sources: ids.map((id) => source(id, { last_modified: dayAgo })),
      revisions: [
        {
          version: 3,
          at: dayAgo,
          edited_by: "distill",
          delta_summary: "Delta",
          incoming_source_ids: ["a", "b"],
        },
      ],
      citationState: "stripped-empty",
    });
    await user.click(screen.getByText("笔记信息"));
    expect(
      screen.getByText("编辑已清除引用。重新整理即可恢复"),
    ).toBeInTheDocument();
    expect(screen.getByText("2 条新增记忆")).toBeInTheDocument();
    expect(screen.getByText("显示全部 6 个来源")).toBeInTheDocument();
    // No English abbreviations leak into the Chinese panel.
    expect(screen.queryByText(/d ago/)).toBeNull();
    expect(screen.getAllByText(/昨天|天前/).length).toBeGreaterThan(0);
  });

  it("localizes labels, counters, badges, and diagnostics in zh-Hant", async () => {
    await i18n.changeLanguage("zh-Hant");
    const { user } = renderInfo({
      sourceCount: 1,
      sources: [source("mem-a", { source_agent: null })],
      citations: [cite(1, 1, "mem-a"), cite(2, 2, "mem-a", { status: "unverified" })],
      citationState: "cited",
    });
    expect(
      screen.getByText("0 條反向連結 · 0 個修訂 · 1 個來源"),
    ).toBeInTheDocument();
    await user.click(screen.getByText("筆記資訊"));
    const headings = screen.getAllByRole("heading", { level: 4 });
    expect(headings.map((h) => h.textContent)).toEqual(["來源"]);
    const row = screen.getByTestId("page-info-source-row");
    expect(within(row).getByText("未驗證")).toBeInTheDocument();
    expect(within(row).getByText("未知代理")).toBeInTheDocument();
    expect(within(row).getByText("記憶")).toBeInTheDocument();
    expect(screen.getByText("引用：2（1 未驗證）")).toBeInTheDocument();
  });

  it("localizes the mismatch diagnostic in zh-Hant", async () => {
    await i18n.changeLanguage("zh-Hant");
    const { user } = renderInfo({ citationState: "stripped-mismatch" });
    await user.click(screen.getByText("筆記資訊"));
    expect(
      screen.getByText("引用資料不相符。重新整理即可修復"),
    ).toBeInTheDocument();
  });

  it("localizes revision loading, failure, retry, and edited re-distill confirmation", async () => {
    const translations = [
      {
        language: "en",
        loading: "Loading revision history…",
        error: "Could not load revision history.",
        retry: "Retry",
        confirm: "Update this note from its sources? Your edits may be replaced.",
      },
      {
        language: "zh-Hans",
        loading: "正在加载修订记录…",
        error: "无法加载修订记录。",
        retry: "重试",
        confirm: "要按来源更新此笔记吗？您的编辑可能会被替换。",
      },
      {
        language: "zh-Hant",
        loading: "正在載入修訂紀錄…",
        error: "無法載入修訂紀錄。",
        retry: "重試",
        confirm: "要依來源更新此筆記嗎？您的編輯可能會被取代。",
      },
    ] as const;

    for (const copy of translations) {
      await i18n.changeLanguage(copy.language);
      const { unmount } = renderInfo({ revisionsLoading: true });
      expect(screen.getByText(copy.loading)).toBeInTheDocument();
      unmount();

      const onRetryRevisions = vi.fn();
      const { user: retryUser, unmount: unmountError } = renderInfo({
        revisionsError: true,
        onRetryRevisions,
      });
      expect(screen.getByRole("alert")).toHaveTextContent(copy.error);
      expect(screen.getByRole("button", { name: copy.retry })).toBeInTheDocument();
      expect(i18n.t("pageDetail.redistillEditedConfirm")).toBe(copy.confirm);
      await retryUser.click(screen.getByRole("button", { name: copy.retry }));
      expect(onRetryRevisions).toHaveBeenCalledOnce();
      unmountError();
    }
  });
  it("labels revisions by writer instead of showing the raw slug", async () => {
    await i18n.changeLanguage("en");
    const at = Math.floor(Date.now() / 1000);
    const { user } = renderInfo({
      revisions: [
        { version: 4, at, edited_by: "agent_refresh", agent: "claude-code", char_delta: 71, sources_added: 2, sources_removed: 0, delta_summary: "+mem_a; +71 chars" },
        { version: 3, at, edited_by: "agent_refresh", char_delta: -3, sources_added: 0, sources_removed: 1 },
        { version: 2, at, edited_by: "distill", delta_summary: "Synthesized from 3 sources" },
        { version: 1, at, edited_by: "migration_99" },
      ],
    });
    await user.click(screen.getByText("Note info"));
    await user.click(screen.getByRole("button", { name: "Show all 4 revisions" }));
    const rows = screen.getAllByRole("listitem").filter((li) => li.className.includes("note-revision-entry"));
    expect(rows.map((row) => row.querySelector(".note-revision-meta")?.textContent)).toEqual([
      expect.stringContaining("Updated by Claude Code"),
      expect.stringContaining("Updated by an agent"),
      expect.stringContaining("Wenlan compiled it from sources"),
      expect.stringContaining("Updated"),
    ]);
    expect(screen.getByText("+71 characters · 2 sources added")).toBeInTheDocument();
    expect(screen.getByText("-3 characters · 1 source removed")).toBeInTheDocument();
    // Old entries show their English summary for an English UI.
    expect(screen.getByText("Synthesized from 3 sources")).toBeInTheDocument();
    expect(screen.queryByText(/agent_refresh|distill\b|migration_99|mem_a/)).toBeNull();
  });

  it("localizes the revision label and summary, and drops old English summaries", async () => {
    await i18n.changeLanguage("zh-Hant");
    const at = Math.floor(Date.now() / 1000);
    const { user } = renderInfo({
      revisions: [
        { version: 3, at, edited_by: "agent_refresh", agent: "claude-code", char_delta: 71, sources_added: 2, sources_removed: 0 },
        { version: 2, at, edited_by: "re_distill", delta_summary: "+mem_a; +71 chars (re_distill)" },
      ],
    });
    await user.click(screen.getByText("筆記資訊"));
    const meta = document.querySelectorAll(".note-revision-meta");
    expect(meta[0]).toHaveTextContent("Claude Code 更新");
    expect(meta[1]).toHaveTextContent("文瀾重新整理");
    expect(screen.getByText("+71 字 · 新增 2 個來源")).toBeInTheDocument();
    expect(screen.queryByText(/mem_a/)).toBeNull();
    expect(document.querySelectorAll(".note-revision-summary")).toHaveLength(1);
  });
});
