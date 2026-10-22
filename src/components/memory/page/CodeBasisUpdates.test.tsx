// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { BasisPendingEntry, CodeBasisPendingEntry, CodeBasisRef, ResolvedRouting } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listBasisPending: vi.fn(),
  listCodeBasisPending: vi.fn(),
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
  listBasisPending,
  listCodeBasisPending,
  readCodeLines,
  readGitBlobLines,
  removePageCodeBasis,
} from "../../../lib/tauri";
import { BasisUpdatesSummary } from "./BasisUpdatesSummary";
import { CodeBasisUpdatesSection } from "./CodeBasisUpdatesSection";

const ref = (over: Partial<CodeBasisRef> = {}): CodeBasisRef => ({
  repo_root: "/repo", path: "src/parser.rs", line_start: 10, line_end: 24, state: "pending", changed_at: 5, ...over,
});

const entry = (over: Partial<CodeBasisPendingEntry> = {}): CodeBasisPendingEntry => ({
  page_id: "note-1",
  title: "Parser notes",
  pending: [ref(), ref({ path: "src/gone.rs", state: "unavailable", reason: "file_missing" })],
  ...over,
});

const routing = (source: string): ResolvedRouting => ({
  everyday: { source, model: null, mode: "auto", pin: null },
  synthesis: { source: "none", model: null, mode: "auto", pin: null },
  pool: { anthropic: { configured: source === "anthropic", everyday_model: null, synthesis_model: null }, external: null, on_device: null },
});

const basisEntry: BasisPendingEntry = {
  basis_page_id: "pricing",
  title: "Pricing",
  state: "pending",
  changed_version: 4,
  citers: [{ page_id: "note-2", title: "Launch plan", minor_only: false }],
};

function renderWith(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return { client, ...render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>) };
}

beforeEach(() => {
  // A daemon without the routing endpoint: no background check, so every changed row is a person's.
  vi.mocked(getResolvedRouting).mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  for (const fn of [listBasisPending, listCodeBasisPending, confirmPageCodeBasis, removePageCodeBasis,
    getPageCodeBasisCited, getResolvedRouting, readCodeLines, readGitBlobLines]) vi.mocked(fn).mockReset();
});

describe("BasisUpdatesSummary with code", () => {
  it("adds one line with the number of notes whose code changed, and opens the review", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([]);
    vi.mocked(listCodeBasisPending).mockResolvedValue([
      entry(),
      entry({ page_id: "note-2", title: "Lexer notes" }),
    ]);
    const onOpen = vi.fn();
    renderWith(<BasisUpdatesSummary onOpen={onOpen} />);

    const list = await screen.findByRole("list", { name: "Code changed" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent("Code behind 2 pages changed");
    expect(listCodeBasisPending).toHaveBeenCalledWith(null);
    const action = within(list).getByRole("button", { name: "Review" });
    expect(action).toHaveAccessibleDescription("Code behind 2 pages changed");
    fireEvent.click(action);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("says 1 note for one note and stacks under the source note lines in one notice", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([basisEntry]);
    vi.mocked(listCodeBasisPending).mockResolvedValue([entry()]);
    const { container } = renderWith(<BasisUpdatesSummary onOpen={vi.fn()} />);

    const list = await screen.findByRole("list", { name: "Source updates" });
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
    const rows = within(list).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent("“Pricing” changed. 1 page to confirm");
    expect(rows[1]).toHaveTextContent("Code behind 1 page changed");
    expect(container.querySelectorAll(".note-review-notice")).toHaveLength(1);
  });

  it("counts only notes whose changed code needs a person once a model looks in the background", async () => {
    vi.mocked(getResolvedRouting).mockResolvedValue(routing("anthropic"));
    vi.mocked(listBasisPending).mockResolvedValue([]);
    vi.mocked(listCodeBasisPending).mockResolvedValue([
      entry({ page_id: "quiet", pending: [ref(), ref({ path: "b.rs", ai_verdict: "stale", explanation: "Now 48." })] }),
      entry({ page_id: "loud", pending: [ref({ ai_verdict: "needs_human", explanation: "On purpose?" })] }),
    ]);
    renderWith(<BasisUpdatesSummary onOpen={vi.fn()} />);

    const list = await screen.findByRole("list", { name: "Code changed" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    expect(within(list).getByRole("listitem")).toHaveTextContent("Code behind 1 page changed");
  });

  it("renders nothing when neither kind is waiting, or when the code read fails", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([]);
    vi.mocked(listCodeBasisPending).mockResolvedValueOnce([entry({ pending: [ref({ state: "ok" })] })]);
    const first = renderWith(<BasisUpdatesSummary onOpen={vi.fn()} />);
    await waitFor(() => expect(listCodeBasisPending).toHaveBeenCalled());
    await waitFor(() => expect(first.client.isFetching()).toBe(0));
    expect(first.container).toBeEmptyDOMElement();
    first.unmount();

    vi.mocked(listCodeBasisPending).mockRejectedValue(new Error("offline"));
    const second = renderWith(<BasisUpdatesSummary onOpen={vi.fn()} />);
    await waitFor(() => expect(listCodeBasisPending).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(second.client.isFetching()).toBe(0));
    expect(second.container).toBeEmptyDOMElement();
  });
});

describe("CodeBasisUpdatesSection in the review panel", () => {
  it("lists one row per note and opens it to the lines that need a look", async () => {
    const onPageClick = vi.fn();
    renderWith(<CodeBasisUpdatesSection entries={[entry()]} onPageClick={onPageClick} />);

    const group = screen.getByRole("region", { name: "Code changed" });
    expect(group).toHaveAttribute("id", "review-code-basis-updates");
    expect(within(group).getByRole("button", { name: "Parser notes" })).toHaveAttribute("aria-expanded", "false");
    expect(await within(group).findByText("2 code references")).toBeInTheDocument();
    expect(within(group).queryByText(/src\/parser.rs/)).toBeNull();

    fireEvent.click(within(group).getByRole("button", { name: "Parser notes" }));
    expect(within(group).getByRole("button", { name: "Parser notes" })).toHaveAttribute("aria-expanded", "true");
    expect(within(group).getByText("The code this page relies on changed: src/parser.rs lines 10–24")).toBeInTheDocument();
    expect(within(group).getByText("Code not found: src/gone.rs (the file is gone)")).toBeInTheDocument();

    fireEvent.click(within(group).getByRole("button", { name: "Open page" }));
    expect(onPageClick).toHaveBeenCalledWith("note-1");
  });

  it("confirms a row and asks for another look when the code changed again", async () => {
    vi.mocked(confirmPageCodeBasis)
      .mockResolvedValueOnce({ outcome: "conflict", message: "again" })
      .mockResolvedValueOnce({ outcome: "confirmed", code_basis: ref({ state: "ok" }) });
    renderWith(<CodeBasisUpdatesSection entries={[entry()]} />);

    fireEvent.click(screen.getByRole("button", { name: "Parser notes" }));
    fireEvent.click(await screen.findByRole("button", { name: "Confirm no impact" }));

    expect(await screen.findByText("The code changed again, please look once more.")).toBeInTheDocument();
    expect(confirmPageCodeBasis).toHaveBeenCalledWith("note-1", expect.objectContaining({ path: "src/parser.rs", line_start: 10 }));

    fireEvent.click(screen.getByRole("button", { name: "Confirm no impact" }));
    await waitFor(() => expect(confirmPageCodeBasis).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByText(/changed again/)).toBeNull());
  });

  it("stops relying on a row that cannot be found and refreshes the list", async () => {
    vi.mocked(removePageCodeBasis).mockResolvedValue({ removed: true });
    const { client } = renderWith(<CodeBasisUpdatesSection entries={[entry()]} />);
    const invalidate = vi.spyOn(client, "invalidateQueries");

    fireEvent.click(screen.getByRole("button", { name: "Parser notes" }));
    fireEvent.click(screen.getByRole("button", { name: "Stop relying on this code" }));

    await waitFor(() => expect(removePageCodeBasis).toHaveBeenCalledWith("note-1", expect.objectContaining({ path: "src/gone.rs" })));
    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["code-basis-pending"] }));
  });

  it("shows the lines side by side from a row", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: "0123456789abcdef0123456789abcdef01234567", confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["old();"]);
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
    renderWith(<CodeBasisUpdatesSection entries={[entry()]} />);

    fireEvent.click(screen.getByRole("button", { name: "Parser notes" }));
    fireEvent.click(await screen.findByRole("button", { name: "See code" }));

    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByTestId("code-basis-before")).toHaveTextContent("old();");
    expect(await within(dialog).findByTestId("code-basis-after")).toHaveTextContent("new();");
  });

  it("renders nothing without entries", () => {
    const { container } = renderWith(<CodeBasisUpdatesSection entries={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});
