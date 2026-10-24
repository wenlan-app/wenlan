// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { CodeBasisRef, Page, ResolvedRouting } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPendingRevisions: vi.fn(),
  listRefinements: vi.fn(),
  confirmPageCodeBasis: vi.fn(),
  removePageCodeBasis: vi.fn(),
  getPageCodeBasisCited: vi.fn(),
  getResolvedRouting: vi.fn(),
  readCodeLines: vi.fn(),
  readGitBlobLines: vi.fn(),
}));

import {
  confirmPageCodeBasis,
  getPageCodeBasisCited,
  getResolvedRouting,
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

const routing = (source: string): ResolvedRouting => ({
  everyday: { source, model: null, mode: "auto", pin: null },
  synthesis: { source: "none", model: null, mode: "auto", pin: null },
  pool: { anthropic: { configured: source === "anthropic", everyday_model: null, synthesis_model: null }, external: null, on_device: null },
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
  // A daemon without the routing endpoint: no background check, so every changed row is a person's.
  vi.mocked(getResolvedRouting).mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  vi.mocked(listPendingRevisions).mockReset();
  vi.mocked(listRefinements).mockReset();
  vi.mocked(confirmPageCodeBasis).mockReset();
  vi.mocked(removePageCodeBasis).mockReset();
  vi.mocked(getPageCodeBasisCited).mockReset();
  vi.mocked(getResolvedRouting).mockReset();
  vi.mocked(readCodeLines).mockReset();
  vi.mocked(readGitBlobLines).mockReset();
});

describe("NoteReviewNotice with a code basis", () => {
  it("says which file changed in one line with one button, and decides in the dialog", async () => {
    const { onEditToConfirm } = renderNotice({ ...basePage, code_basis: [row()] });

    expect(await screen.findByText("src/parser.rs changed, needs a look")).toBeInTheDocument();
    // One way in, nothing else on the notice.
    expect(screen.getAllByRole("button")).toHaveLength(1);
    // Code on its own is a thing to look at, like a changed note.
    expect(screen.getByRole("status")).toHaveClass("note-review-notice--attention");

    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog", { name: "src/parser.rs lines 10–24" });
    // A single row has no step count.
    expect(within(dialog).queryByText("1 / 1")).toBeNull();
    expect(within(dialog).getByRole("button", { name: "No impact" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Stop tracking" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Edit this page yourself" }));
    expect(onEditToConfirm).toHaveBeenCalledWith(expect.objectContaining({ path: "src/parser.rs", line_start: 10 }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("counts several rows in one line", async () => {
    renderNotice({
      ...basePage,
      code_basis: [row({ path: "a.rs" }), row({ path: "b.rs", state: "unavailable", reason: "file_missing" })],
    });

    expect(await screen.findByText("2 code spots behind this page need a look")).toBeInTheDocument();
    expect(screen.getAllByRole("button")).toHaveLength(1);
    expect(screen.queryByText(/a\.rs/)).toBeNull();
  });

  it("shows what the background check concluded in the dialog, and leaves a stale row to the steep", async () => {
    renderNotice({
      ...basePage,
      code_basis: [
        row({ path: "a.rs", ai_verdict: "needs_human", explanation: "The note says 32 bits are kept on purpose; the code now keeps 48." }),
        row({ path: "b.rs", ai_verdict: "stale", explanation: "The constant is now 48, not 32." }),
        row({ path: "c.rs" }),
      ],
    });

    // Without a model nobody will look at c.rs, so it counts; the stale b.rs is the next steep's.
    expect(await screen.findByText("2 code spots behind this page need a look")).toBeInTheDocument();
    expect(screen.queryByText(/^AI looked:/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog", { name: "a.rs lines 10–24" });
    expect(within(dialog).getByText("1 / 2")).toBeInTheDocument();
    expect(within(dialog).getByText("AI looked: The note says 32 bits are kept on purpose; the code now keeps 48."))
      .toBeInTheDocument();
  });

  it("with a model in the background, shows only the row the check could not settle", async () => {
    vi.mocked(getResolvedRouting).mockResolvedValue(routing("anthropic"));
    renderNotice({
      ...basePage,
      code_basis: [
        row({ path: "a.rs", ai_verdict: "needs_human", explanation: "The note says 32 bits are kept on purpose; the code now keeps 48." }),
        row({ path: "b.rs", ai_verdict: "stale", explanation: "The constant is now 48, not 32." }),
        row({ path: "c.rs" }),
      ],
    });

    await waitFor(() => expect(getResolvedRouting).toHaveBeenCalled());
    expect(await screen.findByText("a.rs changed, needs a look")).toBeInTheDocument();
  });

  it("shows nothing at all while every changed row is the background check's to look at", async () => {
    vi.mocked(getResolvedRouting).mockResolvedValue(routing("on_device"));
    const { container, client } = renderNotice({
      ...basePage,
      code_basis: [row(), row({ path: "b.rs", ai_verdict: "stale", explanation: "The constant is now 48, not 32." })],
    });

    await waitFor(() => expect(getResolvedRouting).toHaveBeenCalled());
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(container).toBeEmptyDOMElement();
  });

  it("shows an unjudged row when the daemon has no model at all", async () => {
    vi.mocked(getResolvedRouting).mockResolvedValue(routing("basic"));
    renderNotice({ ...basePage, code_basis: [row()] });

    expect(await screen.findByText("src/parser.rs changed, needs a look")).toBeInTheDocument();
  });

  it("says code that cannot be found by its reason in the dialog, without reading it", async () => {
    vi.mocked(removePageCodeBasis).mockResolvedValue({ removed: true });
    renderNotice({
      ...basePage,
      code_basis: [
        row({ path: "a.rs", state: "unavailable", reason: "file_missing" }),
        row({ path: "b.rs", state: "unavailable", reason: "repo_missing" }),
        row({ path: "c.rs", state: "unavailable", reason: "outside_repo" }),
      ],
    });

    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    let dialog = await screen.findByRole("dialog", { name: "a.rs lines 10–24" });
    expect(within(dialog).getByText("Code not found: a.rs (the file is gone)")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "No impact" })).toBeNull();
    expect(readCodeLines).not.toHaveBeenCalled();
    expect(getPageCodeBasisCited).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Later" }));
    dialog = await screen.findByRole("dialog", { name: "b.rs lines 10–24" });
    expect(within(dialog).getByText("Code not found: b.rs (the repository is gone)")).toBeInTheDocument();
    expect(removePageCodeBasis).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Stop tracking" }));
    await waitFor(() => expect(removePageCodeBasis).toHaveBeenCalledWith("page-1", expect.objectContaining({ path: "b.rs" })));
    dialog = await screen.findByRole("dialog", { name: "c.rs lines 10–24" });
    expect(within(dialog).getByText("Cannot read code: c.rs")).toBeInTheDocument();
    expect(within(dialog).getByText("3 / 3")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Later" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("leaves a deleted file to the steep, which updates the page", async () => {
    const { container, client } = renderNotice({
      ...basePage,
      code_basis: [row({ state: "unavailable", reason: "file_deleted" })],
    });

    await waitFor(() => expect(listPendingRevisions).toHaveBeenCalled());
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(container).toBeEmptyDOMElement();
  });

  it("asks a person about lines that are no longer in the file, even with a model in the background", async () => {
    vi.mocked(getResolvedRouting).mockResolvedValue(routing("anthropic"));
    renderNotice({ ...basePage, code_basis: [row({ reason: "lines_not_found" })] });

    await waitFor(() => expect(getResolvedRouting).toHaveBeenCalled());
    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog", { name: "src/parser.rs lines 10–24" });
    expect(within(dialog).getByText("The lines this page relied on are no longer in the file")).toBeInTheDocument();
  });

  it("names a missing file in the one line", async () => {
    renderNotice({ ...basePage, code_basis: [row({ state: "unavailable", reason: "file_missing" })] });

    expect(await screen.findByText("Can’t find src/parser.rs")).toBeInTheDocument();
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

  it("confirms the lines side by side, refreshes what shows them, and goes on to the next row", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: "0123456789abcdef0123456789abcdef01234567", confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["old();"]);
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
    vi.mocked(confirmPageCodeBasis).mockResolvedValue({ outcome: "confirmed", code_basis: row({ state: "ok" }) });
    const { client } = renderNotice({
      ...basePage,
      code_basis: [row({ line_start: 3, line_end: 3 }), row({ path: "b.rs" })],
    });
    const invalidate = vi.spyOn(client, "invalidateQueries");

    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    let dialog = await screen.findByRole("dialog", { name: "src/parser.rs line 3" });
    expect(await within(dialog).findByTestId("code-basis-after")).toHaveTextContent("new();");

    fireEvent.click(within(dialog).getByRole("button", { name: "No impact" }));
    await waitFor(() => expect(confirmPageCodeBasis).toHaveBeenCalledWith("page-1", expect.objectContaining({
      repo_root: "/repo", path: "src/parser.rs", line_start: 3, line_end: 3,
    })));
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["code-basis-pending"] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["page", "page-1"] });

    dialog = await screen.findByRole("dialog", { name: "b.rs lines 10–24" });
    fireEvent.click(within(dialog).getByRole("button", { name: "No impact" }));
    await waitFor(() => expect(confirmPageCodeBasis).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("stays on the row and asks for another look when the code changed again", async () => {
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: null, confirmed_at: 1, state: "pending" });
    vi.mocked(confirmPageCodeBasis).mockResolvedValue({ outcome: "conflict", message: "the file changed again" });
    renderNotice({ ...basePage, code_basis: [row()] });

    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "No impact" }));

    expect(await screen.findByText("The code changed again, please look once more.")).toHaveAttribute("role", "status");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    // The lines are read afresh for the second look.
    await waitFor(() => expect(readCodeLines).toHaveBeenCalledTimes(2));
  });

  it("reports a failed confirm and a failed stop in their own words", async () => {
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: null, confirmed_at: 1, state: "pending" });
    vi.mocked(confirmPageCodeBasis).mockRejectedValue(new Error("offline"));
    vi.mocked(removePageCodeBasis).mockRejectedValue(new Error("offline"));
    renderNotice({ ...basePage, code_basis: [row()] });

    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "No impact" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Could not confirm, please try again.");

    fireEvent.click(within(dialog).getByRole("button", { name: "Stop tracking" }));
    await waitFor(() => expect(within(dialog).getByRole("alert")).toHaveTextContent("Could not stop tracking it, please try again."));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});
