// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { BasisDiff, BasisRef, Page } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPendingRevisions: vi.fn(),
  listRefinements: vi.fn(),
  getPageBasisDiff: vi.fn(),
  confirmPageBasis: vi.fn(),
  unmarkPageBasis: vi.fn(),
}));

import {
  confirmPageBasis,
  getPageBasisDiff,
  listPendingRevisions,
  listRefinements,
  unmarkPageBasis,
} from "../../../lib/tauri";
import NoteReviewNotice from "./NoteReviewNotice";
import { useBasisActions } from "./useBasisActions";

const basePage: Page = {
  id: "page-1", title: "Pricing summary", summary: null, content: "", entity_id: null,
  domain: null, source_memory_ids: ["memory-1"], version: 1, status: "active",
  created_at: "2026-10-08", last_compiled: "2026-10-08", last_modified: "2026-10-08",
};

const pending = (over: Partial<BasisRef> = {}): BasisRef => ({
  page_id: "basis-a", title: "Pricing", state: "pending", from_version: 1, to_version: 3, ...over,
});

const diff: BasisDiff = {
  from_version: 1,
  to_version: 3,
  from_content: "The plan costs 10 dollars.",
  to_content: "The plan costs 12 dollars.",
  edits: [
    { version: 2, edited_by: "agent_refresh", at: 1_760_000_000, minor: false, delta_summary: "+mem_a; +23 chars; +5 words (agent_refresh)" },
    { version: 3, edited_by: "manual_edit", at: 1_760_000_100, minor: true, delta_summary: "Fixed a typo" },
  ],
};

function Harness({ page, handlers }: {
  page: Page;
  handlers: { onEditToConfirm: (basis: BasisRef) => void; onUpdateFromSources: () => void };
}) {
  const actions = useBasisActions();
  return <NoteReviewNotice
    page={page}
    onReview={vi.fn()}
    onShowSources={vi.fn()}
    basis={{ actions, ...handlers }}
  />;
}

function renderNotice(page: Page) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const handlers = { onEditToConfirm: vi.fn(), onUpdateFromSources: vi.fn() };
  const view = render(<QueryClientProvider client={client}><Harness page={page} handlers={handlers} /></QueryClientProvider>);
  return { ...view, client, ...handlers };
}

beforeEach(() => {
  vi.mocked(listPendingRevisions).mockResolvedValue([]);
  vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
});

afterEach(() => {
  cleanup();
  vi.mocked(listPendingRevisions).mockReset();
  vi.mocked(listRefinements).mockReset();
  vi.mocked(getPageBasisDiff).mockReset();
  vi.mocked(confirmPageBasis).mockReset();
  vi.mocked(unmarkPageBasis).mockReset();
});

describe("NoteReviewNotice with a basis", () => {
  it("treats a has_basis block as a plain source update, not an attention state", async () => {
    renderNotice({ ...basePage, stale_reason: "source_updated", refresh_blocked_reason: "has_basis" });

    const notice = await screen.findByRole("status");
    expect(notice).toHaveTextContent("A source changed; the current page was kept.");
    expect(notice).not.toHaveTextContent("could not finish");
    expect(notice).not.toHaveClass("note-review-notice--attention");
  });

  it("keeps the blocked wording for a citation gate", async () => {
    renderNotice({ ...basePage, stale_reason: "source_updated", refresh_blocked_reason: "citation_gate" });

    const notice = await screen.findByRole("status");
    expect(notice).toHaveTextContent("The update could not finish; the current page was kept.");
    expect(notice).toHaveClass("note-review-notice--attention");
  });

  it("draws no empty review row above the basis lines", async () => {
    const { container } = renderNotice({ ...basePage, basis: [pending()] });

    await screen.findByText(/Source “Pricing” has an update to confirm/);
    expect(container.querySelector(".note-review-notice__content")).toBeNull();
    expect(container.querySelectorAll(".note-review-notice > .note-review-notice__actions")).toHaveLength(0);
  });

  it("shows one line per changed basis with its visible and tucked-away actions", async () => {
    const { onEditToConfirm, onUpdateFromSources } = renderNotice({
      ...basePage,
      basis: [pending({ minor_only: true }), { page_id: "basis-b", title: "Fine", state: "ok", from_version: 1 }],
    });

    expect(await screen.findByText(/Source “Pricing” has an update to confirm \(minor edit\)/)).toBeInTheDocument();
    expect(screen.queryByText(/Fine/)).toBeNull();
    expect(screen.getByRole("button", { name: "See changes" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Confirm no impact" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Edit it yourself" }));
    expect(onEditToConfirm).toHaveBeenCalledWith(expect.objectContaining({ page_id: "basis-a" }));

    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Update from sources" }));
    expect(onUpdateFromSources).toHaveBeenCalledTimes(1);
  });

  it("shows what changed and confirms at the version the dialog showed", async () => {
    vi.mocked(getPageBasisDiff).mockResolvedValue({ outcome: "available", diff });
    vi.mocked(confirmPageBasis).mockResolvedValue({ outcome: "confirmed", basis: pending({ state: "ok" }) });
    renderNotice({ ...basePage, basis: [pending()] });

    fireEvent.click(await screen.findByRole("button", { name: "See changes" }));
    const dialog = await screen.findByRole("dialog", { name: "Changes since you last checked" });
    expect(await within(dialog).findByText(/version 1 to 3/)).toBeInTheDocument();
    const text = within(dialog).getByTestId("basis-diff-text");
    expect(text.querySelector("del")).toHaveTextContent("10");
    expect(text.querySelector("ins")).toHaveTextContent("12");
    // Who made each edit reads as a person would say it. The daemon's English
    // summary carries writer slugs and source ids, so it stays out.
    const edits = within(dialog).getByRole("region", { name: "Edits" });
    expect(within(edits).getAllByRole("listitem")[0]).toHaveTextContent(/^Updated by an agent · /);
    expect(within(edits).getAllByRole("listitem")[1]).toHaveTextContent(/^You · /);
    expect(edits).not.toHaveTextContent(/agent_refresh|manual_edit|human|mem_a|chars|Fixed a typo/);

    fireEvent.click(within(dialog).getByRole("button", { name: "Confirm no impact" }));
    await waitFor(() => expect(confirmPageBasis).toHaveBeenCalledWith("page-1", "basis-a", 3));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("names the agent behind an edit when the source note recorded one", async () => {
    const named = { ...diff, edits: [{ ...diff.edits[0], agent: "claude-code" }, diff.edits[1]] };
    vi.mocked(getPageBasisDiff).mockResolvedValue({ outcome: "available", diff: named });
    renderNotice({ ...basePage, basis: [pending()] });

    fireEvent.click(await screen.findByRole("button", { name: "See changes" }));
    const dialog = await screen.findByRole("dialog", { name: "Changes since you last checked" });
    const edits = await within(dialog).findByRole("region", { name: "Edits" });
    expect(within(edits).getAllByRole("listitem")[0]).toHaveTextContent(/^Updated by Claude Code · /);
    expect(within(edits).getAllByRole("listitem")[1]).toHaveTextContent(/^You · /);
  });

  it("confirms from the notice at the version the note already knows", async () => {
    vi.mocked(confirmPageBasis).mockResolvedValue({ outcome: "confirmed", basis: pending({ state: "ok" }) });
    renderNotice({ ...basePage, basis: [pending()] });

    fireEvent.click(await screen.findByRole("button", { name: "Confirm no impact" }));
    await waitFor(() => expect(confirmPageBasis).toHaveBeenCalledWith("page-1", "basis-a", 3));
  });

  it("asks for another look when the source note changed again", async () => {
    vi.mocked(confirmPageBasis).mockResolvedValue({ outcome: "conflict", message: "moved on" });
    renderNotice({ ...basePage, basis: [pending()] });

    fireEvent.click(await screen.findByRole("button", { name: "Confirm no impact" }));
    expect(await screen.findByText("“Pricing” changed again. Please take another look.")).toBeInTheDocument();
  });

  it("offers only 'stop using' when the source note is archived, deleted or elsewhere", async () => {
    vi.mocked(unmarkPageBasis).mockResolvedValue(undefined as never);
    renderNotice({
      ...basePage,
      basis: [
        pending({ page_id: "a", title: "Old", state: "archived" }),
        pending({ page_id: "d", title: "Gone", state: "deleted", to_version: null }),
        pending({ page_id: "u", title: "", state: "unavailable" }),
      ],
    });

    expect(await screen.findByText("May be out of date: “Old” was archived")).toBeInTheDocument();
    expect(screen.getByText("May be out of date: “Gone” was deleted")).toBeInTheDocument();
    expect(screen.getByText(/moved to another space/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "See changes" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Confirm no impact" })).toBeNull();

    const stops = screen.getAllByRole("button", { name: "Stop using as a source" });
    expect(stops).toHaveLength(3);
    fireEvent.click(stops[0]);
    await waitFor(() => expect(unmarkPageBasis).toHaveBeenCalledWith("page-1", "a"));
  });
});
