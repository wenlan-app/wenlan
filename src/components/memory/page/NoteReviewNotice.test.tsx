// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { Page, PendingRevisionItem, RefinementProposalSummary } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", () => ({
  listPendingRevisions: vi.fn(),
  listRefinements: vi.fn(),
  getResolvedRouting: vi.fn().mockResolvedValue(null),
}));

import { listPendingRevisions, listRefinements } from "../../../lib/tauri";
import NoteReviewNotice from "./NoteReviewNotice";

const basePage: Page = {
  id: "page-1", title: "Current page", summary: null, content: "", entity_id: null,
  domain: null, source_memory_ids: ["memory-1", "memory-2"], version: 1, status: "active",
  created_at: "2026-10-08", last_compiled: "2026-10-08", last_modified: "2026-10-08",
};

function revision(target: string, kind: PendingRevisionItem["target_kind"] = "memory"): PendingRevisionItem {
  return {
    target_source_id: target, revision_source_id: `revision-${target}`, revision_content: "proposal",
    source_agent: "codex", last_modified: 1, target_kind: kind,
  };
}

function proposal(
  id: string,
  action: RefinementProposalSummary["action"],
  source_ids: string[] = [],
  payload: RefinementProposalSummary["payload"] = null,
): RefinementProposalSummary {
  return { id, action, source_ids, payload, confidence: 0.9, created_at: "2026-10-08 00:00:00" };
}

function renderNotice(page: Page = basePage, overrides: { onReview?: (id?: string) => void; onShowSources?: () => void; disabled?: boolean } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onReview = overrides.onReview ?? vi.fn();
  const onShowSources = overrides.onShowSources ?? vi.fn();
  const view = render(<QueryClientProvider client={client}><NoteReviewNotice
    page={page} onReview={onReview} onShowSources={onShowSources} disabled={overrides.disabled}
  /></QueryClientProvider>);
  return { ...view, client, onReview, onShowSources };
}

afterEach(() => {
  cleanup();
  vi.mocked(listPendingRevisions).mockReset();
  vi.mocked(listRefinements).mockReset();
});

describe("NoteReviewNotice", () => {
  it("matches exact page and linked source targets and opens the first relevant review item", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([
      revision("page-1", "page"), revision("memory-1"), revision("memory-elsewhere"),
      revision("page-other", "page"),
    ]);
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [
        proposal("merge-this", "page_merge", ["page-1", "page-other"], {
        action: "page_merge", left_page_id: "page-1", right_page_id: "page-other",
        source_overlap: 0.5, source_overlap_ratio: 0.5,
      }),
      proposal("archive-this", "page_keep_or_archive", ["page-1"], {
        action: "page_keep_or_archive", page_id: "page-1", source_count: 2,
      }),
      proposal("other-page", "page_keep_or_archive", ["page-2"], {
        action: "page_keep_or_archive", page_id: "page-2", source_count: 2,
      }),
      proposal("contradiction", "detect_contradiction", ["memory-2", "memory-1"]),
      proposal("lint", "lint_repair_review", ["memory-1"], {
        action: "lint_repair_review", check_id: "check", occurrence_digest: "occurrence",
        owner_binding_digest: "owner", issue: "Issue", choices: [], suggested_research_queries: [],
      }),
      proposal("unlinked", "detect_contradiction", ["memory-elsewhere"]),
      proposal("unconfirmed-noise", "vocab_promote", ["memory-1"], {
        action: "vocab_promote", kind: "entity", old_value: "ignored",
      }),
    ] });
    const onReview = vi.fn();
    renderNotice({ ...basePage, review_status: "unconfirmed" }, { onReview });

    expect(await screen.findByRole("status")).toHaveTextContent("6 items need review");
    expect(screen.getByText("Page revisions (1)")).toBeInTheDocument();
    expect(screen.getByText("Related source issues (3)")).toBeInTheDocument();
    expect(screen.getByText("Page suggestions (2)")).toBeInTheDocument();
    expect(screen.getByRole("status")).not.toHaveTextContent("other-page");
    expect(screen.getByRole("status")).not.toHaveTextContent("unlinked");
    expect(screen.getByRole("status")).not.toHaveTextContent("unconfirmed");
    fireEvent.click(screen.getByRole("button", { name: "Review changes" }));
    expect(onReview).toHaveBeenCalledWith("revision:page-1");
  });

  it("skips a malformed matching page suggestion and opens the next actionable one", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([]);
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [
      proposal("id-malformed", "page_merge", ["page-1"]),
      proposal("id-exact-page", "page_merge", ["page-1", "page-2"], {
        action: "page_merge", left_page_id: "page-1", right_page_id: "page-2",
        source_overlap: 0.5, source_overlap_ratio: 0.5,
      }),
      proposal("id-prefix-only", "page_merge", ["page-10"]),
    ] });
    const onReview = vi.fn();
    renderNotice(basePage, { onReview });

    fireEvent.click(await screen.findByRole("button", { name: "Review changes" }));
    expect(onReview).toHaveBeenCalledWith("refinement:id-exact-page");
    expect(screen.getByText("Page suggestions (1)")).toBeInTheDocument();
    expect(screen.getByRole("status")).not.toHaveTextContent("id-prefix-only");
  });

  it("ignores malformed matching refinements before counting or opening the notice", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([]);
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [
      proposal("one-source-contradiction", "detect_contradiction", ["memory-1"]),
      proposal("mismatched-lint", "lint_repair_review", ["memory-1"], {
        action: "vocab_promote", kind: "entity", old_value: "ignored",
      }),
      proposal("cannot-keep", "page_keep_or_archive", ["page-1"], {
        action: "page_keep_or_archive", page_id: "page-1", source_count: 2, allowed_actions: ["dismiss"],
      }),
    ] });
    const { container } = renderNotice();
    await waitFor(() => expect(listRefinements).toHaveBeenCalledOnce());
    expect(container.querySelector(".note-review-notice")).toBeNull();
  });

  it("shows source conflict, blocked update, and source change states with source navigation", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([]);
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
    const onShowSources = vi.fn();
    const { rerender } = renderNotice({ ...basePage, stale_reason: "source_conflict" }, { onShowSources });
    expect(await screen.findByText("A source conflict needs attention.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "View details" }));
    expect(onShowSources).toHaveBeenCalledOnce();

    rerender(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <NoteReviewNotice page={{ ...basePage, stale_reason: "source_updated", refresh_blocked_reason: "citation_gate" }}
        onReview={vi.fn()} onShowSources={onShowSources} />
    </QueryClientProvider>);
    expect(await screen.findByText("The update could not finish; the current page was kept.")).toBeInTheDocument();

    cleanup();
    const view = renderNotice({ ...basePage, stale_reason: "source_updated" }, { onShowSources });
    expect(await screen.findByText("A source changed; the current page was kept.")).toBeInTheDocument();
    expect(screen.getByRole("status")).not.toHaveTextContent("Updating");
    view.unmount();
  });

  it("does not repeat generic source-updated copy when a page revision is pending", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([revision("page-1", "page")]);
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
    renderNotice({ ...basePage, stale_reason: "source_updated" });
    expect(await screen.findByRole("button", { name: "Review changes" })).toBeInTheDocument();
    expect(screen.queryByText("A source changed; the current page was kept.")).toBeNull();
  });

  it("keeps matching cached items visible after a partial query error and retries both lists", async () => {
    vi.mocked(listPendingRevisions).mockRejectedValue(new Error("offline"));
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
    const { client } = renderNotice();
    client.setQueryData(["pending-revisions", "note-context"], [revision("memory-1")]);
    // The preloaded data is stale, so the failed request must retain its matching row.
    client.setQueryDefaults(["pending-revisions", "note-context"], { staleTime: 0 });
    client.invalidateQueries({ queryKey: ["pending-revisions", "note-context"] });

    expect(await screen.findByText("Could not check for pending review items.")).toBeInTheDocument();
    expect(screen.getByText("Related source issues (1)")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Review changes" })).toBeInTheDocument();
    vi.mocked(listPendingRevisions).mockResolvedValue([]);
    const callsBeforeRetry = vi.mocked(listPendingRevisions).mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(listPendingRevisions).toHaveBeenCalledTimes(callsBeforeRetry + 1));
  });

  it("marks a capped response incomplete and never renders an all-clear notice", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue(Array.from({ length: 500 }, (_, index) => revision(`other-${index}`)));
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
    const onReview = vi.fn();
    renderNotice(basePage, { onReview });

    expect(await screen.findByText("Only part of the review list was checked.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Review changes" })).toBeNull();
    expect(screen.queryByText("1 item needs review")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open review queue" }));
    expect(onReview).toHaveBeenCalledWith();
  });

  it("renders no persistent strip when the page has no pending or stale work", async () => {
    vi.mocked(listPendingRevisions).mockResolvedValue([]);
    vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
    const { container } = renderNotice();
    await waitFor(() => expect(listRefinements).toHaveBeenCalledOnce());
    expect(container.querySelector(".note-review-notice")).toBeNull();
  });
});
