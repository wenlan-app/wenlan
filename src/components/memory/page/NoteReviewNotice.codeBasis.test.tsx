// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { CodeBasisRef, Page } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPendingRevisions: vi.fn(),
  listRefinements: vi.fn(),
  confirmPageCodeBasis: vi.fn(),
  removePageCodeBasis: vi.fn(),
  getPageCodeBasisCited: vi.fn(),
  readCodeLines: vi.fn(),
  readGitBlobLines: vi.fn(),
}));

import {
  confirmPageCodeBasis,
  getPageCodeBasisCited,
  listPendingRevisions,
  listRefinements,
  readCodeLines,
  readGitBlobLines,
  removePageCodeBasis,
} from "../../../lib/tauri";
import NoteReviewNotice from "./NoteReviewNotice";
import { useCodeBasisActions } from "./useCodeBasisActions";

const basePage: Page = {
  id: "page-1", title: "Parser notes", summary: null, content: "", entity_id: null,
  domain: null, source_memory_ids: ["memory-1"], version: 1, status: "active",
  created_at: "2026-10-08", last_compiled: "2026-10-08", last_modified: "2026-10-08",
};

const row = (over: Partial<CodeBasisRef> = {}): CodeBasisRef => ({
  repo_root: "/repo", path: "src/parser.rs", line_start: 10, line_end: 24, state: "pending", changed_at: 5, ...over,
});

function Harness({ page, onEditToConfirm }: { page: Page; onEditToConfirm: (row: CodeBasisRef) => void }) {
  const actions = useCodeBasisActions();
  return <NoteReviewNotice
    page={page}
    onReview={vi.fn()}
    onShowSources={vi.fn()}
    codeBasis={{ actions, onEditToConfirm }}
  />;
}

function renderNotice(page: Page) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onEditToConfirm = vi.fn();
  const view = render(<QueryClientProvider client={client}>
    <Harness page={page} onEditToConfirm={onEditToConfirm} />
  </QueryClientProvider>);
  return { ...view, client, onEditToConfirm };
}

beforeEach(() => {
  vi.mocked(listPendingRevisions).mockResolvedValue([]);
  vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
});

afterEach(() => {
  cleanup();
  vi.mocked(listPendingRevisions).mockReset();
  vi.mocked(listRefinements).mockReset();
  vi.mocked(confirmPageCodeBasis).mockReset();
  vi.mocked(removePageCodeBasis).mockReset();
  vi.mocked(getPageCodeBasisCited).mockReset();
  vi.mocked(readCodeLines).mockReset();
  vi.mocked(readGitBlobLines).mockReset();
});

describe("NoteReviewNotice with a code basis", () => {
  it("says which lines changed, with the visible and tucked-away actions", async () => {
    const { onEditToConfirm } = renderNotice({ ...basePage, code_basis: [row()] });

    expect(await screen.findByText("The code this page relies on changed: src/parser.rs lines 10–24"))
      .toBeInTheDocument();
    expect(screen.getByRole("button", { name: "See code" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Confirm no impact" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem")).toBeNull();
    // Code on its own is a thing to look at, like a changed note.
    expect(screen.getByRole("status")).toHaveClass("note-review-notice--attention");

    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    expect(screen.getByRole("menuitem", { name: "Stop relying on this code" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Edit this page yourself" }));
    expect(onEditToConfirm).toHaveBeenCalledWith(expect.objectContaining({ path: "src/parser.rs", line_start: 10 }));
  });

  it("names a single line without a range", async () => {
    renderNotice({ ...basePage, code_basis: [row({ line_start: 7, line_end: 7 })] });

    expect(await screen.findByText("The code this page relies on changed: src/parser.rs line 7")).toBeInTheDocument();
  });

  it("explains code that cannot be found by its reason and offers only a way out", async () => {
    renderNotice({
      ...basePage,
      code_basis: [
        row({ path: "a.rs", state: "unavailable", reason: "file_missing" }),
        row({ path: "b.rs", state: "unavailable", reason: "repo_missing" }),
        row({ path: "c.rs", state: "unavailable", reason: "outside_repo" }),
        row({ path: "d.rs", state: "unavailable", reason: "too_large" }),
        row({ path: "e.rs", state: "unavailable" }),
      ],
    });

    expect(await screen.findByText("Code not found: a.rs (the file is gone)")).toBeInTheDocument();
    expect(screen.getByText("Code not found: b.rs (the repository is gone)")).toBeInTheDocument();
    expect(screen.getByText("Cannot read code: c.rs")).toBeInTheDocument();
    expect(screen.getByText("Cannot read code: d.rs")).toBeInTheDocument();
    expect(screen.getByText("Cannot read code: e.rs")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "See code" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Confirm no impact" })).toBeNull();
    expect(screen.getAllByRole("button", { name: "Stop relying on this code" })).toHaveLength(5);
  });

  it("shows nothing for lines that are up to date or only moved", async () => {
    const { container, client } = renderNotice({
      ...basePage,
      code_basis: [row({ state: "ok" }), row({ path: "moved.rs", state: "moved" })],
    });

    await waitFor(() => expect(listPendingRevisions).toHaveBeenCalled());
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(container).toBeEmptyDOMElement();
  });

  it("confirms the lines that changed and refreshes what shows them", async () => {
    vi.mocked(confirmPageCodeBasis).mockResolvedValue({ outcome: "confirmed", code_basis: row({ state: "ok" }) });
    const { client } = renderNotice({ ...basePage, code_basis: [row()] });
    const invalidate = vi.spyOn(client, "invalidateQueries");

    fireEvent.click(await screen.findByRole("button", { name: "Confirm no impact" }));

    await waitFor(() => expect(confirmPageCodeBasis).toHaveBeenCalledWith("page-1", expect.objectContaining({
      repo_root: "/repo", path: "src/parser.rs", line_start: 10, line_end: 24,
    })));
    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["code-basis-pending"] }));
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["page", "page-1"] });
    expect(screen.queryByText(/changed again/)).toBeNull();
  });

  it("asks for another look when the code changed again", async () => {
    vi.mocked(confirmPageCodeBasis).mockResolvedValue({ outcome: "conflict", message: "the file changed again" });
    renderNotice({ ...basePage, code_basis: [row()] });

    fireEvent.click(await screen.findByRole("button", { name: "Confirm no impact" }));

    expect(await screen.findByText("The code changed again, please look once more.")).toHaveAttribute("role", "status");
  });

  it("reports a failed confirm and a failed stop in their own words", async () => {
    vi.mocked(confirmPageCodeBasis).mockRejectedValue(new Error("offline"));
    vi.mocked(removePageCodeBasis).mockRejectedValue(new Error("offline"));
    renderNotice({ ...basePage, code_basis: [row()] });

    fireEvent.click(await screen.findByRole("button", { name: "Confirm no impact" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not confirm, please try again.");

    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop relying on this code" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Could not stop relying on it, please try again."));
  });

  it("stops relying on the lines", async () => {
    vi.mocked(removePageCodeBasis).mockResolvedValue({ removed: true });
    renderNotice({ ...basePage, code_basis: [row({ state: "unavailable", reason: "file_missing" })] });

    fireEvent.click(await screen.findByRole("button", { name: "Stop relying on this code" }));

    await waitFor(() => expect(removePageCodeBasis).toHaveBeenCalledWith("page-1", expect.objectContaining({
      path: "src/parser.rs", line_start: 10, line_end: 24,
    })));
  });

  it("opens the lines side by side from the notice and confirms from there", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: "0123456789abcdef0123456789abcdef01234567", confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["old();"]);
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
    vi.mocked(confirmPageCodeBasis).mockResolvedValue({ outcome: "confirmed", code_basis: row({ state: "ok" }) });
    renderNotice({ ...basePage, code_basis: [row({ line_start: 3, line_end: 3 })] });

    fireEvent.click(await screen.findByRole("button", { name: "See code" }));
    const dialog = await screen.findByRole("dialog", { name: "src/parser.rs line 3" });
    expect(await within(dialog).findByTestId("code-basis-after")).toHaveTextContent("new();");

    fireEvent.click(within(dialog).getByRole("button", { name: "Confirm no impact" }));
    await waitFor(() => expect(confirmPageCodeBasis).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
