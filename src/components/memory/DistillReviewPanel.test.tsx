// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DistillReviewPanel from "./DistillReviewPanel";
import { DISTILL_REVIEW_SESSION_QUERY_KEY } from "./pages/pageReviewSignals";
import {
  EXAMPLE_REVIEW_ITEMS,
  exampleReviewLabel,
  isExampleReviewItem,
  seedReviewExampleCaches,
} from "./reviewExamples";
import type {
  DistillReviewResponse,
  MemoryItem,
  PendingRevisionItem,
} from "../../lib/tauri";

vi.mock("../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../lib/tauri")>("../../lib/tauri");
  return {
    ...actual,
    distillReview: vi.fn(),
    listPendingRevisions: vi.fn(),
    acceptPendingRevision: vi.fn(),
    dismissPendingRevision: vi.fn(),
    listRefinements: vi.fn(),
    acceptRefinement: vi.fn(),
    rejectRefinement: vi.fn(),
    listUnconfirmedMemories: vi.fn(),
    confirmMemory: vi.fn(),
    deleteMemory: vi.fn(),
    getMemoryDetail: vi.fn(),
    getEntityDetail: vi.fn(),
    getPage: vi.fn(),
    getPageSources: vi.fn(),
    search: vi.fn(),
    redistillPage: vi.fn(),
    listRecentChanges: vi.fn(),
    clipboardWrite: vi.fn(),
  };
});

import {
  distillReview,
  listPendingRevisions,
  acceptPendingRevision,
  dismissPendingRevision,
  listRefinements,
  acceptRefinement,
  rejectRefinement,
  listUnconfirmedMemories,
  confirmMemory,
  deleteMemory,
  getMemoryDetail,
  getEntityDetail,
  getPage,
  getPageSources,
  search,
  redistillPage,
  clipboardWrite,
  listRecentChanges,
} from "../../lib/tauri";

function renderPanel(props: Partial<React.ComponentProps<typeof DistillReviewPanel>> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const onBack = props.onBack ?? vi.fn();
  const onPageClick = props.onPageClick ?? vi.fn();
  const onMemoryClick = props.onMemoryClick ?? vi.fn();
  const onOpenActivity = props.onOpenActivity ?? vi.fn();
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={client}>
      <DistillReviewPanel initialReviewItemId={props.initialReviewItemId} onBack={onBack} onPageClick={onPageClick} onMemoryClick={onMemoryClick} onOpenActivity={onOpenActivity} />
    </QueryClientProvider>,
  );
  return { user, client, onBack, onPageClick, onMemoryClick, onOpenActivity };
}

function truncateForTest(value: string, max: number): string {
  if (max <= 0) return "";
  if (value.length <= max) return value;
  if (max <= 3) return ".".repeat(max);
  return `${value.slice(0, max - 3).trimEnd()}...`;
}

function memory(overrides: Partial<MemoryItem> & Pick<MemoryItem, "source_id" | "title" | "content">): MemoryItem {
  return {
    summary: null,
    memory_type: null,
    domain: null,
    source_agent: null,
    confidence: null,
    confirmed: true,
    pinned: false,
    supersedes: null,
    last_modified: 1_760_000_000,
    chunk_count: 1,
    ...overrides,
  };
}

function revision(
  overrides: Partial<PendingRevisionItem> & Pick<PendingRevisionItem, "target_source_id" | "revision_content">,
): PendingRevisionItem {
  return {
    revision_source_id: `${overrides.target_source_id}_rev`,
    source_agent: "claude-code",
    last_modified: 1_760_000_000,
    target_kind: "memory",
    ...overrides,
  };
}

const fallbackSource =
  "Fallback content label should appear when no title or entity exists, and this preview detail must stay visible in the review panel source list even when it becomes the cluster title.";
const fallbackSecondSource =
  "A second fallback source confirms the preview keeps the first two contents.";

const reviewPayload: DistillReviewResponse = {
  pages_created: 0,
  scoped: false,
  created_ids: [],
  pending: [
    {
      source_ids: ["mem_1", "mem_2"],
      contents: [
        "This is a detailed source memory about temporal page refresh behavior.",
        "A second source memory adds routing context for the distill review panel.",
      ],
      entity_id: "entity_temporal",
      entity_name: "Temporal refresh",
      space: "Engineering",
      estimated_tokens: 180,
      centroid_embedding: [0.1, 0.2],
      existing_page_id: "page_temporal",
      existing_page_title: "Temporal page refresh",
      new_memory_count: 1,
    },
    {
      source_ids: ["mem_3"],
      contents: [fallbackSource, fallbackSecondSource],
      estimated_tokens: 80,
    },
  ],
  stale_pages: [
    {
      page_id: "page_stale",
      title: "Retrieval Pipeline",
      summary: "Source memories changed after the page compiled.",
      source_memory_ids: ["mem_old"],
      sources_updated_count: 3,
      stale_reason: "source_updated",
      user_edited: false,
    },
  ],
  stale_truncated: true,
  orphan_topics: [{ label: "Vector clocks", count: 4 }],
};

beforeEach(async () => {
  const actual = await vi.importActual<typeof import("../../lib/tauri")>("../../lib/tauri");
  vi.mocked(getPage).mockImplementation(actual.getPage);
  vi.mocked(getPageSources).mockImplementation(actual.getPageSources);
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(distillReview).mockResolvedValue(reviewPayload);
  vi.mocked(listRecentChanges).mockResolvedValue([]);
  vi.mocked(listPendingRevisions).mockResolvedValue([]);
  vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
  vi.mocked(listUnconfirmedMemories).mockResolvedValue([]);
  vi.mocked(confirmMemory).mockResolvedValue(undefined);
  vi.mocked(deleteMemory).mockResolvedValue(undefined);
  vi.mocked(acceptPendingRevision).mockResolvedValue({
    target_source_id: "mem_target",
    revision_source_id: "mem_target_rev",
    wrote: true,
  });
  vi.mocked(dismissPendingRevision).mockResolvedValue({
    target_source_id: "mem_target",
    wrote: true,
  });
  vi.mocked(acceptRefinement).mockResolvedValue({ id: "prop_1", action_applied: "entity_merge" });
  vi.mocked(rejectRefinement).mockResolvedValue({ id: "prop_1" });
  vi.mocked(getMemoryDetail).mockResolvedValue(
    memory({ source_id: "mem_target", title: "Target memory", content: "Prefers npm for installs" }),
  );
  vi.mocked(search).mockResolvedValue([]);
  vi.mocked(redistillPage).mockResolvedValue({
    status: "ok",
    updated: true,
    hint: null,
  } as any);
  vi.mocked(getEntityDetail).mockResolvedValue({
    entity: {
      id: "ent_1",
      name: "Visual Studio Code",
      entity_type: "tool",
      domain: null,
      source_agent: null,
      confidence: null,
      confirmed: true,
      created_at: 0,
      updated_at: 0,
      memory_count: 1,
      status: "established",
      established_by: "manual",
    },
    observations: [],
    relations: [],
  });
});

describe("DistillReviewPanel", () => {
  it("accepts vocabulary through the existing command and refreshes affected entity caches", async () => {
    vi.mocked(listRefinements).mockResolvedValueOnce({ proposals: [{ id: "vocab", action: "vocab_promote", source_ids: ["ent_1"], payload: { action: "vocab_promote", kind: "entity", old_value: "research-method" }, confidence: 0.9, created_at: "2026-09-13T10:00:00Z" }] }).mockResolvedValue({ proposals: [] });
    const { user, client } = renderPanel();
    client.setQueryData(["entities", "all"], [{ id: "ent_1", entity_type: "concept" }]);
    client.setQueryData(["entityDetail", "ent_1"], { entity: { entity_type: "concept" } });
    await user.click(await screen.findByRole("button", { name: "Review research-method" }));
    const button = screen.getByRole("button", { name: "Add to vocabulary" });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    await waitFor(() => expect(acceptRefinement).toHaveBeenCalledWith("vocab"));
    await waitFor(() => expect(client.getQueryState(["entities", "all"])?.isInvalidated).toBe(true));
    expect(client.getQueryState(["entityDetail", "ent_1"])?.isInvalidated).toBe(true);
  });
  it("loads the page review once on mount", async () => {
    const { client } = renderPanel();

    await waitFor(() => {
      expect(distillReview).toHaveBeenCalledTimes(1);
    });
    expect(client.getQueryData(DISTILL_REVIEW_SESSION_QUERY_KEY)).toEqual(reviewPayload);
    expect(client.getQueryDefaults(DISTILL_REVIEW_SESSION_QUERY_KEY)).toMatchObject({
      gcTime: Infinity,
      staleTime: Infinity,
    });
    expect(await screen.findByRole("heading", { name: "Review" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^refresh$/i })).toBeInTheDocument();
  });

  it("keeps background discovery out of decisions and offers Activity as its destination", async () => {
    const { user, onOpenActivity } = renderPanel();

    expect(await screen.findByText("Retrieval Pipeline")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Pages with new sources" })).toBeInTheDocument();
    expect(screen.getByText("Retrieval Pipeline")).toBeInTheDocument();
    expect(screen.getByText(/first 10 stale pages/i)).toBeInTheDocument();
    expect(screen.getByText(/Background discoveries are available in Activity/)).toBeInTheDocument();
    expect(screen.queryByText("Vector clocks")).toBeNull();
    expect(screen.queryByRole("heading", { name: "New page candidates" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "New topics" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Review Temporal page refresh" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Review Vector clocks" })).toBeNull();
    const filters = screen.getByRole("group", { name: "Filter reviews" });
    expect(within(filters).getByRole("button", { name: /^All1$/ })).toBeInTheDocument();
    expect(within(filters).queryByRole("button", { name: /^Candidates/ })).toBeNull();
    expect(within(filters).queryByRole("button", { name: /^Topics/ })).toBeNull();
    expect(screen.queryByText("Coffee routine")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Open Activity" }));
    expect(onOpenActivity).toHaveBeenCalledTimes(1);
    expect(distillReview).toHaveBeenCalledTimes(1);
    expect(acceptRefinement).not.toHaveBeenCalled();
  });

  it("keeps candidate source previews out of review cards", async () => {
    renderPanel();
    expect(await screen.findByText("Retrieval Pipeline")).toBeInTheDocument();
    expect(screen.queryByText(truncateForTest(fallbackSource, 72))).toBeNull();
    expect(screen.queryByText(truncateForTest(fallbackSource, 140))).toBeNull();
    expect(screen.queryByText(fallbackSecondSource)).toBeNull();
  });

  it("opens a stale page card and refreshes it through redistill", async () => {
    const { user, onPageClick } = renderPanel();

    await user.click(
      await screen.findByRole("button", { name: /Review Retrieval Pipeline/ }),
    );

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Page refresh")).toBeInTheDocument();
    expect(within(dialog).getByText("3 sources updated")).toBeInTheDocument();
    // No dismiss verb exists for stale pages, so the dialog never offers one.
    expect(within(dialog).queryByRole("button", { name: "Dismiss" })).toBeNull();

    await user.click(within(dialog).getByRole("button", { name: "Open page" }));
    expect(onPageClick).toHaveBeenCalledWith("page_stale");

    await user.click(within(dialog).getByRole("button", { name: "Refresh page" }));
    await waitFor(() => {
      expect(redistillPage).toHaveBeenCalledWith("page_stale");
    });
  });

  it("keeps a stale page card when the daemon has no model to rebuild it", async () => {
    vi.mocked(redistillPage).mockResolvedValue({ status: "skipped", updated: false, hint: "needs an LLM" });
    vi.mocked(clipboardWrite).mockResolvedValue(undefined);
    const { user } = renderPanel();

    await user.click(
      await screen.findByRole("button", { name: /Review Retrieval Pipeline/ }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Refresh page" }));

    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Copy command" })).toBeInTheDocument());
    expect(within(dialog).queryByRole("button", { name: "Refresh page" })).toBeNull();
    expect(within(dialog).getByText("Page refresh")).toBeInTheDocument();
    expect(within(dialog).getByText(/run \/wenlan steep/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Copy command" }));
    expect(clipboardWrite).toHaveBeenCalledWith("/wenlan steep");
  });

  it("keeps the last successful result visible after refresh failure", async () => {
    vi.mocked(distillReview)
      .mockResolvedValueOnce(reviewPayload)
      .mockRejectedValueOnce(new Error("HTTP POST /api/distill returned 500"));
    const { user } = renderPanel();

    expect(await screen.findByText("Retrieval Pipeline")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /^refresh$/i }));

    expect(await screen.findByText(/HTTP POST \/api\/distill returned 500/)).toBeInTheDocument();
    expect(screen.getByText("Retrieval Pipeline")).toBeInTheDocument();
  });

  it("keeps manual refresh available after the initial load", async () => {
    const { user } = renderPanel();

    await screen.findByText("Retrieval Pipeline");
    await user.click(screen.getByRole("button", { name: /^refresh$/i }));

    expect(distillReview).toHaveBeenCalledTimes(2);
  });
});

describe("DistillReviewPanel review queue", () => {
  it("shows all-caught-up only when the queue and discovery are both empty", async () => {
    vi.mocked(distillReview).mockResolvedValue({
      pages_created: 0,
      scoped: false,
      created_ids: [],
      pending: [],
      stale_pages: [],
      stale_truncated: false,
      orphan_topics: [],
    });
    renderPanel();

    expect(await screen.findByRole("heading", { name: "All caught up" })).toBeInTheDocument();
  });

  it("shows no decisions waiting when only background discoveries remain", async () => {
    vi.mocked(distillReview).mockResolvedValue({ ...reviewPayload, stale_pages: [], stale_truncated: false });
    renderPanel();

    expect(await screen.findByRole("heading", { name: "No decisions waiting" })).toBeInTheDocument();
    expect(screen.getByText(/Background discoveries are available in Activity/)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "All caught up" })).toBeNull();
  });

  it("does not claim all-caught-up when the review queue fails to load, and retry reuses the existing refresh", async () => {
    vi.mocked(distillReview).mockResolvedValue({
      pages_created: 0,
      scoped: false,
      created_ids: [],
      pending: [],
      stale_pages: [],
      stale_truncated: false,
      orphan_topics: [],
    });
    vi.mocked(listRefinements).mockRejectedValue(
      new Error("HTTP GET /api/refinery/queue returned 500"),
    );

    const { user } = renderPanel();

    expect(
      await screen.findByRole("heading", { name: "Couldn't load review items." }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "All caught up" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(listRefinements).toHaveBeenCalledTimes(2);
    expect(distillReview).toHaveBeenCalledTimes(2);
  });

  it("shows an incomplete notice when the decision sources are capped with no actionable items", async () => {
    vi.mocked(distillReview).mockResolvedValue({
      pages_created: 0, scoped: false, created_ids: [], pending: [], stale_pages: [],
      stale_truncated: false, orphan_topics: [],
    });
    vi.mocked(listRefinements).mockResolvedValue({ proposals: Array.from({ length: 50 }, (_, index) => ({
      id: `unsupported-${index}`, action: "suggest_entity" as const, source_ids: ["a", "b"],
      payload: { action: "suggest_entity" as const, name_hint: `Entity ${index}` },
      confidence: 0.8, created_at: "2026-10-08 00:00:00",
    })) });
    renderPanel();

    expect(await screen.findByText("Only part of the review list was checked.")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "All caught up" })).toBeNull();
    expect(screen.queryByRole("button", { name: /to decide/ })).toBeNull();
  });

  it("hides stale pages locally and restores them without making a daemon decision", async () => {
    const { user } = renderPanel();
    await user.click(await screen.findByRole("button", { name: /Review Retrieval Pipeline/ }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Hide" }));

    expect(await screen.findByRole("button", { name: /1 hidden/ })).toBeInTheDocument();
    expect(redistillPage).not.toHaveBeenCalled();
    expect(rejectRefinement).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: /1 hidden/ }));
    await user.click(screen.getByRole("button", { name: "Restore" }));
    expect(await screen.findByRole("button", { name: /Review Retrieval Pipeline/ })).toBeInTheDocument();
  });

  it("shows the queue plus a quiet partial-load notice, no retry button, when items exist alongside a queue error", async () => {
    vi.mocked(distillReview).mockResolvedValue({
      pages_created: 0,
      scoped: false,
      created_ids: [],
      pending: [],
      stale_pages: [],
      stale_truncated: false,
      orphan_topics: [],
    });
    vi.mocked(listRefinements).mockRejectedValue(
      new Error("HTTP GET /api/refinery/queue returned 500"),
    );
    vi.mocked(listPendingRevisions).mockResolvedValue([
      revision({ target_source_id: "mem_target", revision_content: "Prefers pnpm for installs" }),
    ]);

    renderPanel();

    expect(await screen.findByRole("heading", { name: "Memory revisions" })).toBeInTheDocument();
    expect(
      await screen.findByText(/Some review items couldn't load/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Couldn't load review items." })).toBeNull();
  });

  it("renders revision cards with a pending count", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([
      revision({ target_source_id: "mem_target", revision_content: "Prefers pnpm for installs" }),
    ]);
    renderPanel();

    expect(await screen.findByRole("heading", { name: "Memory revisions" })).toBeInTheDocument();
    expect(screen.getByText("2 to decide")).toBeInTheDocument();
    // The card titles itself with the target memory's real name once fetched.
    expect(await screen.findByRole("button", { name: /Review Target memory/ })).toBeInTheDocument();
  });

  it("opens the diff dialog from a card and shows stripped and added words", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([
      revision({ target_source_id: "mem_target", revision_content: "Prefers pnpm for installs" }),
    ]);
    const { user } = renderPanel();

    await user.click(await screen.findByRole("button", { name: /Review Target memory/ }));

    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("heading", { name: "Target memory" })).toBeInTheDocument();
    await waitFor(() => {
      const dels = dialog.querySelectorAll("del");
      const inss = dialog.querySelectorAll("ins");
      expect([...dels].some((el) => el.textContent?.includes("npm"))).toBe(true);
      expect([...inss].some((el) => el.textContent?.includes("pnpm"))).toBe(true);
    });
    // The stale page remains actionable; candidate/topic discovery is excluded.
    expect(within(dialog).getByText("1 of 2")).toBeInTheDocument();
  });

  it("approves the last revision and shows the caught-up pane when nothing else is queued", async () => {
    vi.mocked(distillReview).mockResolvedValue({
      pages_created: 0,
      scoped: false,
      created_ids: [],
      pending: [],
      stale_pages: [],
      stale_truncated: false,
      orphan_topics: [],
    });
    vi.mocked(listPendingRevisions)
      .mockResolvedValueOnce([
        revision({ target_source_id: "mem_target", revision_content: "Prefers pnpm for installs" }),
      ])
      .mockResolvedValue([]);
    const { user } = renderPanel();

    await user.click(await screen.findByRole("button", { name: /Review Target memory/ }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Approve" }));

    await waitFor(() => {
      expect(acceptPendingRevision).toHaveBeenCalledWith("mem_target");
    });
    expect(await within(dialog).findByRole("heading", { name: "Review complete" })).toBeInTheDocument();
    expect(within(dialog).getByText("You can close this review.")).toBeInTheDocument();
  });

  it("finishes after the last decision without advancing into unsupported or background items", async () => {
    vi.mocked(distillReview).mockResolvedValue({ ...reviewPayload, stale_pages: [], stale_truncated: false });
    vi.mocked(listPendingRevisions)
      .mockResolvedValueOnce([
        revision({ target_source_id: "mem_target", revision_content: "Prefers pnpm for installs" }),
      ])
      .mockResolvedValue([]);
    vi.mocked(listRefinements).mockResolvedValueOnce({
      proposals: [{
        id: "cross-space",
        action: "cross_space_discovery",
        source_ids: ["mem_target"],
        payload: { action: "cross_space_discovery", memory_count: 1, spaces: ["Other"] },
        confidence: 0.8,
        created_at: "2026-09-13T10:00:00Z",
      }],
    }).mockResolvedValue({ proposals: [] });
    const { user } = renderPanel();

    await user.click(await screen.findByRole("button", { name: /Review Target memory/ }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Approve" }));

    await waitFor(() => {
      expect(acceptPendingRevision).toHaveBeenCalledWith("mem_target");
    });
    expect(await within(dialog).findByRole("heading", { name: "Review complete" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("heading", { name: "Vector clocks" })).toBeNull();
    expect(within(dialog).queryByRole("heading", { name: "Temporal page refresh" })).toBeNull();
    expect(acceptRefinement).not.toHaveBeenCalled();
  });

  it("dismisses a revision through the dialog", async () => {
    vi.mocked(listPendingRevisions)
      .mockResolvedValueOnce([
        revision({ target_source_id: "mem_target", revision_content: "Prefers pnpm for installs" }),
      ])
      .mockResolvedValue([]);
    const { user } = renderPanel();

    await user.click(await screen.findByRole("button", { name: /Review Target memory/ }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Dismiss" }));

    await waitFor(() => {
      expect(dismissPendingRevision).toHaveBeenCalledWith("mem_target");
    });
  });

  it("advances to the next item after approving", async () => {
    const first = revision({ target_source_id: "mem_a", revision_content: "First revision content" });
    const second = revision({ target_source_id: "mem_b", revision_content: "Second revision content" });
    vi.mocked(listPendingRevisions)
      .mockResolvedValueOnce([first, second])
      .mockResolvedValue([second]);
    vi.mocked(getMemoryDetail).mockImplementation(async (sourceId: string) =>
      memory({ source_id: sourceId, title: `Title ${sourceId}`, content: "old content" }),
    );
    const { user } = renderPanel();

    await user.click(await screen.findByRole("button", { name: /Review Title mem_a/ }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("heading", { name: "Title mem_a" })).toBeInTheDocument();
    // Two revisions and one stale page remain actionable.
    expect(within(dialog).getByText("1 of 3")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "Approve" }));

    expect(await within(dialog).findByRole("heading", { name: "Title mem_b" })).toBeInTheDocument();
    await waitFor(() => {
      expect(acceptPendingRevision).toHaveBeenCalledWith("mem_a");
    });
  });

  it("renders refinement proposals and approves through the daemon verb", async () => {
    vi.mocked(listRefinements)
      .mockResolvedValueOnce({
        proposals: [
          {
            id: "prop_1",
            action: "entity_merge",
            source_ids: ["ent_2", "ent_1"],
            payload: { action: "entity_merge", existing_id: "ent_1", new_id: "ent_2", similarity: 0.94 },
            confidence: 0.94,
            created_at: "2026-07-09T00:00:00Z",
          },
        ],
      })
      .mockResolvedValue({ proposals: [] });
    const { user } = renderPanel();

    expect(await screen.findByRole("heading", { name: "Merges & suggestions" })).toBeInTheDocument();
    // Both entity ids resolve through the same mocked detail, so the card
    // titles itself with the fetched names instead of the bare kind label.
    await user.click(
      await screen.findByRole("button", { name: /look like the same entity/ }),
    );

    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("94% confidence")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Approve" }));

    await waitFor(() => {
      expect(acceptRefinement).toHaveBeenCalledWith("prop_1");
    });
  });

  it("opens the contradiction dialog with before/after panes and resolves it", async () => {
    vi.mocked(listRefinements).mockResolvedValue({
      proposals: [
        {
          id: "ref-contra",
          action: "detect_contradiction",
          source_ids: ["mem-new", "mem-old"],
          payload: { action: "detect_contradiction" },
          confidence: 0.78,
          created_at: "2026-07-09T00:00:00Z",
        },
      ],
    });
    vi.mocked(getMemoryDetail).mockImplementation(async (sourceId: string) =>
      memory({
        source_id: sourceId,
        title: sourceId === "mem-new" ? "New memory" : "Old memory",
        content: sourceId === "mem-new"
          ? "The project stores data in redb."
          : "The project stores data in SQLite.",
      }),
    );
    const { user } = renderPanel();

    await user.click(await screen.findByRole("button", { name: /contradicts/ }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Existing memory")).toBeInTheDocument();
    expect(within(dialog).getByText("Newer memory")).toBeInTheDocument();
    await within(dialog).findByText(/SQLite/);
    await within(dialog).findByText(/redb/);
    expect(within(dialog).getByRole("button", { name: "Keep both" })).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "Resolve" }));
    await waitFor(() => expect(acceptRefinement).toHaveBeenCalledWith("ref-contra"));
    expect(rejectRefinement).not.toHaveBeenCalled();
  });

  it("opens the page-merge strip-off dossier and merges it", async () => {
    vi.mocked(listRefinements).mockResolvedValue({
      proposals: [
        {
          id: "ref-page-merge",
          action: "page_merge",
          source_ids: ["page-keep", "page-absorb"],
          payload: {
            action: "page_merge",
            left_page_id: "page-keep",
            right_page_id: "page-absorb",
            source_overlap: 5,
            source_overlap_ratio: 1.0,
          },
          confidence: 1.0,
          created_at: "2026-07-09T00:00:00Z",
        },
      ],
    });
    vi.mocked(getPage).mockImplementation(async (id: string) => ({
      id,
      title: id === "page-keep" ? "Surviving page" : "Absorbed page",
      content: "Page body",
      summary: null,
      entity_id: null,
      domain: null,
      source_memory_ids: [],
      version: 1,
      status: "active",
      created_at: "2026-07-09T00:00:00Z",
      last_compiled: "2026-07-09T00:00:00Z",
      last_modified: "2026-07-09T00:00:00Z",
    }) as any);
    // The retiring page's 5 sources are a strict subset of the kept page's 6,
    // so the dossier ledger yields no unique retiring sources: safe verdict.
    vi.mocked(getPageSources).mockImplementation(async (id: string) => {
      const ids = id === "page-keep" ? ["m1", "m2", "m3", "m4", "m5", "m6"] : ["m1", "m2", "m3", "m4", "m5"];
      return ids.map((m) => ({
        source: { page_id: id, memory_source_id: m, linked_at: 0 },
        memory: { source_id: m, title: `Source ${m}`, content: `Evidence ${m}` },
      })) as any;
    });
    const { user } = renderPanel();

    await user.click(
      await screen.findByRole("button", { name: /“Surviving page” absorbs “Absorbed page”/ }),
    );

    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/Nothing unique is lost/i);
    expect(within(dialog).getByText("Kept page")).toBeInTheDocument();
    expect(within(dialog).getByText("Retiring page")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: /Merge pages/i }));
    await waitFor(() => expect(acceptRefinement).toHaveBeenCalledWith("ref-page-merge"));
  });

  it("shows a before/after relation pair for relation conflicts and approves", async () => {
    vi.mocked(listRefinements).mockResolvedValue({
      proposals: [
        {
          id: "ref-relation",
          action: "relation_conflict",
          source_ids: ["rel-new", "rel-old"],
          payload: {
            action: "relation_conflict",
            existing_id: "rel-old",
            new_id: "rel-new",
            from: "Lucian",
            to: "Zed",
            old_type: "EVALUATES",
            new_type: "USES_DAILY",
          },
          confidence: 0.82,
          created_at: "2026-07-09T00:00:00Z",
        },
      ],
    });
    const { user } = renderPanel();

    await user.click(await screen.findByRole("button", { name: /Lucian → Zed/ }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Current relation")).toBeInTheDocument();
    expect(within(dialog).getByText("Proposed relation")).toBeInTheDocument();
    expect(within(dialog).getByText(/EVALUATES/)).toBeInTheDocument();
    expect(within(dialog).getByText(/USES_DAILY/)).toBeInTheDocument();
    // The relation ids must never be fetched as memories.
    expect(getMemoryDetail).not.toHaveBeenCalledWith("rel-new");

    await user.click(within(dialog).getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(acceptRefinement).toHaveBeenCalledWith("ref-relation"));
  });

  it("walks the review queue revisions first, then conflicts, then page items", async () => {
    vi.mocked(distillReview).mockResolvedValue({
      ...reviewPayload,
      pending: [],
      stale_pages: [],
      stale_truncated: false,
      orphan_topics: [],
    });
    vi.mocked(listPendingRevisions).mockResolvedValue([
      revision({ target_source_id: "mem_target", revision_content: "Prefers pnpm for installs" }),
    ]);
    vi.mocked(listRefinements).mockResolvedValue({
      proposals: [
        {
          id: "ref-merge",
          action: "entity_merge",
          source_ids: ["ent_2", "ent_1"],
          payload: { action: "entity_merge", existing_id: "ent_1", new_id: "ent_2", similarity: 0.94 },
          confidence: 0.94,
          created_at: "2026-07-09T00:00:00Z",
        },
        {
          id: "ref-page-merge",
          action: "page_merge",
          source_ids: ["page-keep", "page-absorb"],
          payload: null,
          confidence: 1.0,
          created_at: "2026-07-09T00:00:00Z",
        },
        {
          id: "ref-contra",
          action: "detect_contradiction",
          source_ids: ["mem-new", "mem-old"],
          payload: { action: "detect_contradiction" },
          confidence: 0.78,
          created_at: "2026-07-09T00:00:00Z",
        },
      ],
    });
    const { user } = renderPanel();

    // The dialog walks one ranked list, so each item's position is its rank.
    const expected: Array<[RegExp, string]> = [
      [/Review Target memory/, "1 of 4"],
      [/contradicts/, "2 of 4"],
      [/Page merge/, "3 of 4"],
      [/look like the same entity/, "4 of 4"],
    ];
    for (const [card, position] of expected) {
      await user.click(await screen.findByRole("button", { name: card }));
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText(position)).toBeInTheDocument();
      await user.keyboard("{Escape}");
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    }
  });

  it("keeps new-memory captures off the review page", async () => {
    vi.mocked(listUnconfirmedMemories).mockResolvedValue([
      {
        kind: "memory",
        id: "mem-capture",
        title: "User prefers pnpm over npm",
        snippet: "Stated while setting up the monorepo.",
        timestamp_ms: 1_782_365_080_000,
        badge: { kind: "needs_review" },
      },
    ]);
    renderPanel();

    expect(await screen.findByText(/Background discoveries are available in Activity/)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "New memories" })).toBeNull();
    expect(screen.queryByText("User prefers pnpm over npm")).toBeNull();
    expect(confirmMemory).not.toHaveBeenCalled();
    expect(deleteMemory).not.toHaveBeenCalled();
  });

  it("keeps unsupported proposals out of cards and opens Activity without resolving them", async () => {
    vi.mocked(listRefinements)
      .mockResolvedValueOnce({
        proposals: [
          {
            id: "prop_entity",
            action: "suggest_entity",
            source_ids: ["mem_x", "mem_y"],
            payload: { action: "suggest_entity", name_hint: "Tauri" },
            confidence: 0.8,
            created_at: "2026-07-09T00:00:00Z",
          },
        ],
      })
      .mockResolvedValue({ proposals: [] });
    const { user, onOpenActivity } = renderPanel();

    expect(await screen.findByText(/Background discoveries are available in Activity/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Review Tauri" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Open Activity" }));
    expect(onOpenActivity).toHaveBeenCalledTimes(1);
    expect(acceptRefinement).not.toHaveBeenCalled();
    expect(rejectRefinement).not.toHaveBeenCalled();
  });

  it("keeps discovery hides out of Review's hidden footer", async () => {
    localStorage.setItem(
      "wenlan.review.hidden.v1",
      JSON.stringify([
        { key: "topic:Vector clocks", label: "Vector clocks", kind: "topic", at: Date.now() },
      ]),
    );
    renderPanel();
    await screen.findByText("Retrieval Pipeline");
    expect(screen.queryByRole("button", { name: /1 hidden/ })).toBeNull();
    expect(JSON.parse(localStorage.getItem("wenlan.review.hidden.v1") ?? "[]")).toHaveLength(1);
  });

  it("renders the revisions section's empty state when nothing is pending", async () => {
    renderPanel();

    await screen.findByRole("heading", { name: "Memory revisions" });
    expect(screen.getByText("Nothing waiting for approval.")).toBeInTheDocument();
    expect(screen.queryByText("Coffee routine")).toBeNull();
    expect(
      screen.getByText(/before\/after diff lands here first/),
    ).toBeInTheDocument();
  });

  it("renders the Recent revisions section from listRecentChanges", async () => {
    vi.mocked(listRecentChanges).mockResolvedValue([
      {
        page_id: "page_recent",
        title: "Threads Metrics And Postmortem Schema",
        change_kind: "revised",
        changed_at_ms: Date.now() - 2 * 60 * 60 * 1000,
      },
    ]);
    const { user, onPageClick } = renderPanel();

    await screen.findByRole("heading", { name: "Recent revisions" });
    const row = await screen.findByRole("button", {
      name: /Threads Metrics And Postmortem Schema/,
    });
    await user.click(row);
    expect(onPageClick).toHaveBeenCalledWith("page_recent");
  });
});

describe("DistillReviewPanel review filter", () => {
  const emptyDistill: DistillReviewResponse = {
    pages_created: 0,
    scoped: false,
    created_ids: [],
    pending: [],
    stale_pages: [],
    stale_truncated: false,
    orphan_topics: [],
  };

  it("shows only All and Revisions chips when the queue is empty", async () => {
    vi.mocked(distillReview).mockResolvedValue(emptyDistill);
    renderPanel();

    const group = await screen.findByRole("group", { name: "Filter reviews" });
    expect(within(group).getByRole("button", { name: /^All/ })).toBeInTheDocument();
    expect(within(group).getByRole("button", { name: /^Revisions/ })).toBeInTheDocument();
    expect(within(group).queryByRole("button", { name: /^Conflicts/ })).toBeNull();
    expect(within(group).queryByRole("button", { name: /^Pages/ })).toBeNull();
    expect(within(group).queryByRole("button", { name: /^Merges/ })).toBeNull();
    expect(within(group).queryByRole("button", { name: /^Candidates/ })).toBeNull();
    expect(within(group).queryByRole("button", { name: /^Topics/ })).toBeNull();
  });

  it("filters to a single category, hides other sections, and Show all returns to all after the last item resolves", async () => {
    localStorage.setItem(
      "wenlan.review.hidden.v1",
      JSON.stringify([{ key: "topic:Ghost topic", label: "Ghost topic", kind: "topic", at: Date.now() }]),
    );
    vi.mocked(distillReview).mockResolvedValue(emptyDistill);
    let resolveRefinements!: (
      response: Awaited<ReturnType<typeof listRefinements>>,
    ) => void;
    const pendingRefinements = new Promise<Awaited<ReturnType<typeof listRefinements>>>(
      (resolve) => {
        resolveRefinements = resolve;
      },
    );
    vi.mocked(listRefinements)
      .mockReturnValueOnce(pendingRefinements)
      .mockResolvedValue({ proposals: [] });
    const { user } = renderPanel();

    // Recent changes are independent from the review queue; hold proposals to
    // prove the conflict filter appears only when its own query has resolved.
    expect(await screen.findByRole("heading", { name: "Recent revisions" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Conflicts/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /1 hidden/ })).toBeNull();

    await act(async () => {
      resolveRefinements({
        proposals: [
          {
            id: "prop_conflict",
            action: "relation_conflict",
            source_ids: ["ent_b", "ent_a"],
            payload: {
              action: "relation_conflict",
              existing_id: "ent_a",
              new_id: "ent_b",
              from: "Alice",
              to: "Bob",
              old_type: "manages",
              new_type: "reports_to",
            },
            confidence: 0.7,
            created_at: "2026-07-09T00:00:00Z",
          },
        ],
      });
    });

    const group = await screen.findByRole("group", { name: "Filter reviews" });
    await user.click(await within(group).findByRole("button", { name: /^Conflicts/ }));

    expect(screen.getByRole("heading", { name: "Contradictions & conflicts" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Memory revisions" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Recent revisions" })).toBeNull();
    expect(screen.queryByRole("button", { name: /1 hidden/ })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Review Alice → Bob" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Approve" }));

    await waitFor(() => {
      expect(acceptRefinement).toHaveBeenCalledWith("prop_conflict");
    });
    expect(await screen.findByText("Nothing in this category right now.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Show all" }));

    expect(
      within(screen.getByRole("group", { name: "Filter reviews" })).getByRole("button", {
        name: /^All/,
      }),
    ).toHaveAttribute("aria-pressed", "true");
  });
});

// REVIEW_EXAMPLES_ENABLED is false under Vitest (MODE === "test"), so the
// panel never renders these — covered here as plain unit tests of the
// exported pure functions/fixtures instead.
describe("reviewExamples", () => {
  it("flags only example: ids", () => {
    for (const item of EXAMPLE_REVIEW_ITEMS) {
      expect(isExampleReviewItem(item)).toBe(true);
    }
    expect(
      isExampleReviewItem({
        kind: "revision",
        id: "mem_target",
        targetSourceId: "mem_target",
        targetKind: "memory",
        revisionSourceId: "mem_target_rev",
        content: "content",
        agent: null,
        timestampMs: null,
      }),
    ).toBe(false);
  });

  it("ships exactly one revision and one detect_contradiction refinement sample", () => {
    expect(EXAMPLE_REVIEW_ITEMS).toHaveLength(2);
    const [revision, refinement] = EXAMPLE_REVIEW_ITEMS;
    expect(revision.kind).toBe("revision");
    expect(refinement.kind).toBe("refinement");
    expect(refinement.kind === "refinement" && refinement.action).toBe(
      "detect_contradiction",
    );
    for (const item of EXAMPLE_REVIEW_ITEMS) {
      expect(item.id.startsWith("example:")).toBe(true);
    }
  });

  it("labels each sample with its human title", () => {
    const [revision, refinement] = EXAMPLE_REVIEW_ITEMS;
    expect(exampleReviewLabel(revision)).toBe("Coffee routine");
    expect(exampleReviewLabel(refinement)).toBe("Standup schedule (updated)");
  });

  it("seeds every id the two dialogs read, with no stale gcTime/staleTime", () => {
    const client = new QueryClient();
    seedReviewExampleCaches(client);

    const [revision, refinement] = EXAMPLE_REVIEW_ITEMS;
    expect(revision.kind).toBe("revision");
    const coffeeId = revision.kind === "revision" ? revision.targetSourceId : "";
    expect(refinement.kind).toBe("refinement");
    const [standupNewId, standupOldId] =
      refinement.kind === "refinement" ? refinement.sourceIds : ["", ""];

    for (const sourceId of [coffeeId, standupNewId, standupOldId]) {
      const memory = client.getQueryData<MemoryItem>(["memory-detail", sourceId]);
      expect(memory?.source_id).toBe(sourceId);
      expect(memory?.content.length).toBeGreaterThan(0);

      const summary = client.getQueryData<{ name: string; text: string }>([
        "review-summary",
        "memory",
        sourceId,
      ]);
      expect(summary?.name.length).toBeGreaterThan(0);

      const chain = client.getQueryData<{ chain_depth: number; entries: unknown[] }>([
        "memory-revisions",
        sourceId,
      ]);
      expect(chain).toEqual({ current_source_id: sourceId, chain_depth: 0, entries: [] });
    }
  });
});


describe("note-context review entry", () => {
  it("opens the requested item even beyond the default queue cap", async () => {
    vi.mocked(listPendingRevisions).mockImplementation(async limit => Array.from({length: 80}, (_, index) => revision({target_source_id: `mem_${index}`, revision_content: `Proposal ${index}`})).slice(0, limit));
    vi.mocked(getMemoryDetail).mockImplementation(async id => memory({source_id: id, title: `Memory ${id}`, content: "Original text"}));
    renderPanel({initialReviewItemId: "revision:mem_79"});
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("heading", {name: "Memory mem_79"})).toBeInTheDocument();
    expect(listPendingRevisions).toHaveBeenCalledWith(500);
    expect(listRefinements).toHaveBeenCalledWith(500);
    expect(acceptPendingRevision).not.toHaveBeenCalled();
  });
  it("can open a known matching revision when the other queue source fails", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([revision({target_source_id: "mem_target", revision_content: "Proposal"})]);
    vi.mocked(listRefinements).mockRejectedValue(new Error("offline"));
    renderPanel({initialReviewItemId: "revision:mem_target"});
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });
  it("explains a missing requested item without choosing another", async () => {
    renderPanel({initialReviewItemId: "revision:handled"});
    expect(await screen.findByText(/This item was not found in the loaded queue/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
  it("does not focus an unsupported proposal from page context", async () => {
    vi.mocked(listRefinements).mockResolvedValue({
      proposals: [{
        id: "cross-space",
        action: "cross_space_discovery",
        source_ids: ["mem_target"],
        payload: { action: "cross_space_discovery", memory_count: 1, spaces: ["Other"] },
        confidence: 0.8,
        created_at: "2026-09-13T10:00:00Z",
      }],
    });
    renderPanel({ initialReviewItemId: "refinement:cross-space" });

    expect(await screen.findByText(/This item was not found in the loaded queue/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
  it.each([true, false])("resolving a revision clears both query scopes and refreshes page data (approve=%s)", async approve => {
    const item = revision({target_source_id: "mem_target", revision_content: "Proposal"});
    vi.mocked(listPendingRevisions).mockResolvedValueOnce([item]).mockResolvedValue([]);
    const {user, client} = renderPanel({initialReviewItemId: "revision:mem_target"});
    client.setQueryData(["pending-revisions"], [item]);
    client.setQueryData(["page", "related-page"], {content: "Original"});
    client.setQueryData(["page-revisions", "related-page"], {entries: []});
    const dialog = await screen.findByRole("dialog");
    const action = within(dialog).getByRole("button", {name: approve ? "Approve" : "Dismiss"});
    await waitFor(() => expect(action).toBeEnabled());
    await user.click(action);
    await waitFor(() => expect(approve ? acceptPendingRevision : dismissPendingRevision).toHaveBeenCalledWith("mem_target"));
    await waitFor(() => expect(client.getQueryData(["pending-revisions", "note-context"])).toEqual([]));
    expect(client.getQueryData(["pending-revisions"])).toEqual([]);
    expect(client.getQueryState(["page", "related-page"])?.isInvalidated).toBe(true);
    expect(client.getQueryState(["page-revisions", "related-page"])?.isInvalidated).toBe(true);
  });
});
