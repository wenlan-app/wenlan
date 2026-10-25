// SPDX-License-Identifier: AGPL-3.0-only
//
// A gated page write stages a revision card whose target is a page, not a
// memory. The dialog has to read that target through the page routes: the
// heading is the page's title, the diff's "before" side is the page's current
// body, and the history strip walks the page's own changelog. Asking the
// memory routes about a page id is a guaranteed 404, so these tests also pin
// that the memory queries never fire for a page card.
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import {
  getMemoryDetail,
  getMemoryRevisions,
  getPage,
  getPageRevisions,
} from "../../lib/tauri";
import ReviewDialog, { useReviewItemSummary } from "./ReviewDialog";
import { reviewItemId, type ReviewItem } from "./useReviewQueue";

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  getPage: vi.fn(),
  getPageRevisions: vi.fn(),
  getMemoryDetail: vi.fn(),
  getMemoryRevisions: vi.fn(),
}));

const PAGE_ID = "page_65c2d620-029e-4fa0-8a59-2917f17633b1";
const PAGE_TITLE = "Why the graph lives in libSQL";
const PAGE_BODY = "The knowledge graph lives in libSQL.";
const PROPOSED =
  "The knowledge graph lives in Neo4j, because a dedicated graph engine keeps traversal cheap at depth.";

const pageItem: ReviewItem = {
  kind: "revision",
  id: PAGE_ID,
  targetSourceId: PAGE_ID,
  targetKind: "page",
  revisionSourceId: "mem_c3e1600d7c3a",
  content: PROPOSED,
  agent: "Claude Code",
  timestampMs: 1_760_000_000_000,
};

const memoryItem: ReviewItem = {
  kind: "revision",
  id: "mem_target",
  targetSourceId: "mem_target",
  targetKind: "memory",
  revisionSourceId: "mem_target_rev",
  content: PROPOSED,
  agent: "claude-code",
  timestampMs: 1_760_000_000_000,
};

function ReviewHarness({
  item,
  onOpenPage,
  onOpenMemory,
  onResolve = vi.fn().mockResolvedValue(undefined),
}: {
  readonly item: ReviewItem;
  readonly onOpenPage?: (pageId: string) => void;
  readonly onOpenMemory?: (sourceId: string) => void;
  readonly onResolve?: (args: { item: ReviewItem; approve: boolean }) => Promise<unknown>;
}) {
  const [openId, setOpenId] = useState<string | null>(null);
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <button type="button" onClick={() => setOpenId(reviewItemId(item))}>
        Open review
      </button>
      <ReviewDialog
        items={[item]}
        openId={openId}
        onOpenChange={setOpenId}
        onResolve={onResolve}
        isResolving={false}
        onOpenPage={onOpenPage}
        onOpenMemory={onOpenMemory}
      />
    </QueryClientProvider>
  );
}

async function openDialog(
  item: ReviewItem,
  handlers: {
    onOpenPage?: (pageId: string) => void;
    onOpenMemory?: (sourceId: string) => void;
    onResolve?: (args: { item: ReviewItem; approve: boolean }) => Promise<unknown>;
  } = {},
): Promise<HTMLElement> {
  const user = userEvent.setup();
  render(<ReviewHarness item={item} {...handlers} />);
  await user.click(screen.getByRole("button", { name: "Open review" }));
  return await screen.findByRole("dialog");
}

describe("ReviewDialog page-target revision card", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.mocked(getPage).mockReset();
    vi.mocked(getPageRevisions).mockReset();
    vi.mocked(getMemoryDetail).mockReset();
    vi.mocked(getMemoryRevisions).mockReset();
    vi.mocked(getPage).mockResolvedValue({
      id: PAGE_ID,
      title: PAGE_TITLE,
      summary: "The storage decision.",
      content: PAGE_BODY,
    } as never);
    vi.mocked(getPageRevisions).mockResolvedValue({
      page_id: PAGE_ID,
      current_version: 2,
      entries: [
        {
          version: 2,
          at: 1_760_000_000,
          edited_by: "page_write",
          delta_summary: "Recorded the Neo4j comparison.",
        },
        {
          version: 1,
          at: 1_759_000_000,
          edited_by: "user",
          delta_summary: "First draft.",
        },
      ],
    } as never);
    // A page id is not a memory id; if either memory route is reached the
    // daemon answers 404, so make that loud instead of silent.
    vi.mocked(getMemoryDetail).mockRejectedValue(new Error("memory not found"));
    vi.mocked(getMemoryRevisions).mockRejectedValue(
      new Error("memory not found"),
    );
  });

  it("heads the card with the page title, not the proposed prose", async () => {
    const dialog = await openDialog(pageItem);
    expect(
      await within(dialog).findByRole("heading", { name: PAGE_TITLE }),
    ).toBeVisible();
    expect(within(dialog).queryByRole("heading", { name: /Neo4j/ })).toBeNull();
    expect(within(dialog).getByText("Page revision")).toBeVisible();
    expect(within(dialog).queryByText("Memory revision")).toBeNull();
  });

  it.each([
    ["en", "page", "page_write", "Page update"],
    ["zh-Hant", "page", "page_write", "頁面更新"],
    ["zh-Hans", "page", "page_write", "页面更新"],
    ["en", "page", "claude-code", "proposed by claude-code"],
    ["zh-Hant", "page", "claude-code", "由 claude-code 提議"],
    ["zh-Hans", "page", "claude-code", "由 claude-code 提议"],
    ["en", "memory", "page_write", "proposed by page_write"],
    ["zh-Hant", "memory", "page_write", "由 page_write 提議"],
    ["zh-Hans", "memory", "page_write", "由 page_write 提议"],
  ] as const)("labels %s %s revision source %s in the list and dialog", async (locale, targetKind, agent, label) => {
    await i18n.changeLanguage(locale);
    const item = { ...pageItem, targetKind, agent };
    if (targetKind === "memory") {
      vi.mocked(getMemoryDetail).mockResolvedValue({
        source_id: PAGE_ID, title: PAGE_TITLE, content: PAGE_BODY,
      } as never);
      vi.mocked(getMemoryRevisions).mockResolvedValue({ entries: [] } as never);
    }
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const { result } = renderHook(() => useReviewItemSummary(item), {
      wrapper: ({ children }) => (
        <QueryClientProvider client={queryClient}>
          {children}
        </QueryClientProvider>
      ),
    });
    expect(result.current.reason).toBe(label);

    const dialog = await openDialog(item);
    expect(within(dialog).getByText(label, { exact: true })).toBeVisible();
    if (targetKind === "page" && agent === "page_write") {
      expect(within(dialog).queryByText(i18n.t("review.proposedBy", { agent }), {
        exact: true,
      })).toBeNull();
    }
  });

  it("diffs the page's current body against the proposed prose", async () => {
    const dialog = await openDialog(pageItem);
    await within(dialog).findByRole("heading", { name: PAGE_TITLE });

    // Unified view: the page's current wording is the deleted side.
    await waitFor(() =>
      expect(dialog.querySelector("del")).toHaveTextContent("libSQL."),
    );
    expect(dialog.querySelector("ins")).toHaveTextContent("Neo4j,");
    // The proposed prose adds 11 words and strips the one the page had.
    expect(within(dialog).getByText(/\+11 −1 words/)).toBeVisible();

    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("tab", { name: "Side by side" }));
    const current = within(dialog).getByText("Current").parentElement as HTMLElement;
    expect(current).toHaveTextContent(PAGE_BODY);
    expect(current).not.toHaveTextContent("Neo4j");
  });

  it("shows the page's own changelog and never asks the memory routes", async () => {
    const dialog = await openDialog(pageItem);
    expect(
      await within(dialog).findByText("History · 2 earlier versions"),
    ).toBeVisible();
    expect(within(dialog).getByText(/Recorded the Neo4j comparison\./)).toBeVisible();

    expect(getPageRevisions).toHaveBeenCalledWith(PAGE_ID);
    expect(getMemoryDetail).not.toHaveBeenCalled();
    expect(getMemoryRevisions).not.toHaveBeenCalled();
  });

  it("hides the strip cleanly when the page has no earlier versions", async () => {
    vi.mocked(getPageRevisions).mockResolvedValue({
      page_id: PAGE_ID,
      current_version: 1,
      entries: [],
    } as never);
    const dialog = await openDialog(pageItem);
    await within(dialog).findByRole("heading", { name: PAGE_TITLE });
    await waitFor(() => expect(getPageRevisions).toHaveBeenCalled());
    expect(within(dialog).queryByText(/History ·/)).toBeNull();
  });

  it("names the proposing agent by its display name", async () => {
    const dialog = await openDialog(pageItem);
    await within(dialog).findByRole("heading", { name: PAGE_TITLE });
    expect(within(dialog).getByText("proposed by Claude Code")).toBeInTheDocument();
  });

  it.each([
    ["authored", "en", "Reason: you wrote this page, so AI edits wait for your review"],
    ["authored", "zh-Hant", "原因：你寫過這篇筆記，AI 的修改要等你審核"],
    ["locked", "en", "Reason: this page is set to Ask me first"],
    ["background", "en", "Reason: Wenlan steeped this automatically"],
    ["owned", "en", "Reason: the page has your edits"],
    ["locked", "zh-Hant", "原因：這篇設成了先問我"],
    ["background", "zh-Hans", "原因：文澜自动沉淀"],
    ["owned", "zh-Hant", "原因：筆記有你的修改"],
  ])("says why a %s card needs review (%s)", async (reviewReason, language, text) => {
    await i18n.changeLanguage(language);
    const dialog = await openDialog({ ...pageItem, reviewReason });
    await within(dialog).findByRole("heading", { name: PAGE_TITLE });
    expect(within(dialog).getByTestId("review-reason")).toHaveTextContent(text);
  });

  it.each([undefined, null, "something_new"])(
    "shows no reason line for reviewReason %s",
    async (reviewReason) => {
      const dialog = await openDialog({ ...pageItem, reviewReason });
      await within(dialog).findByRole("heading", { name: PAGE_TITLE });
      expect(within(dialog).queryByTestId("review-reason")).toBeNull();
      expect(within(dialog).getByText("proposed by Claude Code")).toBeInTheDocument();
    },
  );

  it("labels history rows by writer and localizes numeric summaries", async () => {
    vi.mocked(getPageRevisions).mockResolvedValue({
      page_id: PAGE_ID,
      current_version: 4,
      entries: [
        { version: 3, at: 1_760_000_000, edited_by: "agent_refresh", agent: "claude-code", char_delta: 71, sources_added: 2, sources_removed: 0, delta_summary: "+mem_a; +71 chars" },
        { version: 2, at: 1_759_500_000, edited_by: "page_write", delta_summary: "Old English summary." },
        { version: 1, at: 1_759_000_000, edited_by: "manual_edit", char_delta: -4, sources_added: 0, sources_removed: 0 },
      ],
    } as never);
    const dialog = await openDialog(pageItem);
    expect(await within(dialog).findByText("Updated by Claude Code")).toBeVisible();
    expect(within(dialog).getByText("+71 characters · 2 sources added")).toBeVisible();
    expect(within(dialog).getByText("You edited in the app")).toBeVisible();
    expect(within(dialog).getByText("-4 characters")).toBeVisible();
    // An unknown writer is "Updated", never its slug; the old English summary
    // is kept for an English UI.
    expect(within(dialog).getByText("Updated")).toBeVisible();
    expect(within(dialog).getByText(/Old English summary\./)).toBeVisible();
    expect(within(dialog).queryByText(/page_write|agent_refresh|manual_edit/)).toBeNull();
    expect(within(dialog).queryByText(/mem_a/)).toBeNull();
  });

  it("explains and moves on when the daemon discarded an outdated card", async () => {
    const onResolve = vi.fn().mockRejectedValue(
      new Error('HTTP 409: {"error":"page_revision_outdated: page x changed after revision card y was proposed"}'),
    );
    const dialog = await openDialog(pageItem, { onResolve });
    await within(dialog).findByRole("heading", { name: PAGE_TITLE });
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("button", { name: "Approve" }));
    expect(
      await screen.findByText(i18n.t("review.revisionOutdated")),
    ).toBeInTheDocument();
    expect(onResolve).toHaveBeenCalledOnce();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("explains and moves on when a page the card relied on is gone", async () => {
    const onResolve = vi.fn().mockRejectedValue(
      new Error('HTTP 409: {"error":"page_revision_basis_refused: the change relied on note y, and it was archived"}'),
    );
    const dialog = await openDialog(pageItem, { onResolve });
    await within(dialog).findByRole("heading", { name: PAGE_TITLE });
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("button", { name: "Approve" }));
    expect(
      await screen.findByText(i18n.t("review.revisionBasisRefused")),
    ).toBeInTheDocument();
    expect(screen.queryByText(i18n.t("review.revisionOutdated"))).toBeNull();
    expect(onResolve).toHaveBeenCalledOnce();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("offers Open page (not Open memory) and opens the page itself", async () => {
    const onOpenPage = vi.fn();
    const onOpenMemory = vi.fn();
    const dialog = await openDialog(pageItem, { onOpenPage, onOpenMemory });
    await within(dialog).findByRole("heading", { name: PAGE_TITLE });

    expect(
      within(dialog).queryByRole("button", { name: "Open memory" }),
    ).toBeNull();
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("button", { name: "Open page" }));
    expect(onOpenPage).toHaveBeenCalledWith(PAGE_ID);
    expect(onOpenMemory).not.toHaveBeenCalled();
  });

  it("still reads a memory-target card through the memory routes", async () => {
    vi.mocked(getMemoryDetail).mockResolvedValue({
      source_id: "mem_target",
      title: "Graph storage",
      content: PAGE_BODY,
    } as never);
    vi.mocked(getMemoryRevisions).mockResolvedValue({
      current_source_id: "mem_target",
      chain_depth: 2,
      entries: [
        {
          source_id: "mem_target_v1",
          depth: 1,
          title: "Graph storage",
          content_preview: "The knowledge graph lives in a graph database.",
          last_modified: 1_759_000_000,
          source_agent: "claude-code",
        },
      ],
    } as never);

    const dialog = await openDialog(memoryItem);
    expect(
      await within(dialog).findByRole("heading", { name: "Graph storage" }),
    ).toBeVisible();
    await waitFor(() =>
      expect(getMemoryRevisions).toHaveBeenCalledWith("mem_target"),
    );
    expect(getPageRevisions).not.toHaveBeenCalled();
    expect(getPage).not.toHaveBeenCalled();
  });
});

describe("ReviewDialog proposer label", () => {
  it.each([
    ["en", "proposed by Wenlan"],
    ["zh-Hant", "由 Wenlan 提議"],
    ["zh-Hans", "由 Wenlan 提议"],
  ])("shows the app's own writer id as Wenlan (%s)", async (language, label) => {
    await i18n.changeLanguage(language);
    vi.mocked(getPage).mockResolvedValue({ id: PAGE_ID, title: PAGE_TITLE, summary: "", content: PAGE_BODY } as never);
    vi.mocked(getPageRevisions).mockResolvedValue({ page_id: PAGE_ID, current_version: 1, entries: [] } as never);
    const item: ReviewItem = { ...pageItem, agent: "wenlan-app" };
    const dialog = await openDialog(item);
    expect(within(dialog).getByText(label, { exact: true })).toBeVisible();
    expect(within(dialog).queryByText(/wenlan-app/)).toBeNull();
    await i18n.changeLanguage("en");
  });
});
