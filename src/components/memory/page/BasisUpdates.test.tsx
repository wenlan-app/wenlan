// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { BasisDiff, BasisPendingEntry } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listBasisPending: vi.fn(),
  getPageBasisDiff: vi.fn(),
  confirmPageBasis: vi.fn(),
}));

import { confirmPageBasis, getPageBasisDiff, listBasisPending } from "../../../lib/tauri";
import { BasisUpdatesSection } from "./BasisUpdatesSection";
import { BasisUpdatesSummary } from "./BasisUpdatesSummary";

const entry = (over: Partial<BasisPendingEntry> = {}): BasisPendingEntry => ({
  basis_page_id: "pricing",
  title: "Pricing",
  state: "pending",
  changed_version: 4,
  citers: [
    { page_id: "note-1", title: "Launch plan", minor_only: false },
    { page_id: "note-2", title: "Sales deck", minor_only: true },
  ],
  ...over,
});

const diff: BasisDiff = {
  from_version: 2,
  to_version: 4,
  from_content: "Old price.",
  to_content: "New price.",
  edits: [],
};

function renderWith(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return { client, ...render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>) };
}

afterEach(() => {
  cleanup();
  vi.mocked(listBasisPending).mockReset();
  vi.mocked(getPageBasisDiff).mockReset();
  vi.mocked(confirmPageBasis).mockReset();
});

describe("BasisUpdatesSummary on Home", () => {
  it("shows one line per changed source note, with the count of notes, and opens the group", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([
      entry({
        citers: Array.from({ length: 20 }, (_, index) => ({ page_id: `n${index}`, title: `Note ${index}`, minor_only: false })),
      }),
      entry({ basis_page_id: "roadmap", title: "Roadmap", citers: [{ page_id: "n1", title: "Note 1", minor_only: false }] }),
    ]);
    const onOpen = vi.fn();
    renderWith(<BasisUpdatesSummary onOpen={onOpen} />);

    const list = await screen.findByRole("list", { name: "Source updates" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("“Pricing” changed. 20 pages to confirm");
    expect(rows[1]).toHaveTextContent("“Roadmap” changed. 1 page to confirm");
    expect(listBasisPending).toHaveBeenCalledWith(null);

    // A compact neutral notice, not an attention box, with a visible action per line.
    const notice = list.closest(".note-review-notice");
    expect(notice).not.toBeNull();
    expect(notice).not.toHaveClass("note-review-notice--attention");
    const lines = within(list).getAllByRole("button", { name: "Review" });
    expect(lines).toHaveLength(2);
    expect(lines[0]).toHaveAccessibleDescription("“Pricing” changed. 20 pages to confirm");

    fireEvent.click(lines[0]);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("renders nothing when no note is waiting, or only an archived source remains", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([entry({ state: "archived" }), entry({ basis_page_id: "x", citers: [] })]);
    const { container, client } = renderWith(<BasisUpdatesSummary onOpen={vi.fn()} />);

    await waitFor(() => expect(listBasisPending).toHaveBeenCalled());
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when the read fails", async () => {
    vi.mocked(listBasisPending).mockRejectedValue(new Error("offline"));
    const { container, client } = renderWith(<BasisUpdatesSummary onOpen={vi.fn()} />);

    await waitFor(() => expect(listBasisPending).toHaveBeenCalled());
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(container).toBeEmptyDOMElement();
  });
});

describe("BasisUpdatesSection in the review panel", () => {
  it("lists one row per source note and opens it to the notes that rely on it", () => {
    renderWith(<BasisUpdatesSection entries={[entry()]} onPageClick={vi.fn()} />);

    const group = screen.getByRole("region", { name: "Source updates" });
    expect(within(group).getByText("“Pricing” changed. 2 pages to confirm")).toBeInTheDocument();
    expect(within(group).queryByText("Launch plan")).toBeNull();

    fireEvent.click(within(group).getByRole("button", { name: "Show the pages" }));
    expect(within(group).getByText("Launch plan")).toBeInTheDocument();
    expect(within(group).getByText("Sales deck")).toBeInTheDocument();
    expect(within(group).getByText("(minor edit)")).toBeInTheDocument();
    expect(within(group).getByRole("button", { name: "Hide the pages" })).toHaveAttribute("aria-expanded", "true");
  });

  it("confirms a note at the version the source note is now, and can open the note", async () => {
    vi.mocked(confirmPageBasis).mockResolvedValue({ outcome: "confirmed", basis: {} as never });
    const onPageClick = vi.fn();
    renderWith(<BasisUpdatesSection entries={[entry()]} onPageClick={onPageClick} />);

    fireEvent.click(screen.getByRole("button", { name: "Show the pages" }));
    fireEvent.click(screen.getAllByRole("button", { name: "Confirm no impact" })[1]);
    await waitFor(() => expect(confirmPageBasis).toHaveBeenCalledWith("note-2", "pricing", 4));

    fireEvent.click(screen.getByRole("button", { name: "Launch plan" }));
    expect(onPageClick).toHaveBeenCalledWith("note-1");
  });

  it("asks for another look when the source note changed again", async () => {
    vi.mocked(confirmPageBasis).mockResolvedValue({ outcome: "conflict", message: "moved on" });
    renderWith(<BasisUpdatesSection entries={[entry()]} />);

    fireEvent.click(screen.getByRole("button", { name: "Show the pages" }));
    fireEvent.click(screen.getAllByRole("button", { name: "Confirm no impact" })[0]);

    expect(await screen.findByText("“Pricing” changed again. Please take another look.")).toBeInTheDocument();
  });

  it("shows the changes before confirming when the version to confirm is not known", async () => {
    vi.mocked(getPageBasisDiff).mockResolvedValue({ outcome: "available", diff });
    vi.mocked(confirmPageBasis).mockResolvedValue({ outcome: "confirmed", basis: {} as never });
    renderWith(<BasisUpdatesSection entries={[entry({ changed_version: null })]} />);

    fireEvent.click(screen.getByRole("button", { name: "Show the pages" }));
    fireEvent.click(screen.getAllByRole("button", { name: "Confirm no impact" })[0]);

    const dialog = await screen.findByRole("dialog", { name: "Changes since you last checked" });
    expect(confirmPageBasis).not.toHaveBeenCalled();
    fireEvent.click(await within(dialog).findByRole("button", { name: "Confirm no impact" }));
    await waitFor(() => expect(confirmPageBasis).toHaveBeenCalledWith("note-1", "pricing", 4));
  });

  it("renders nothing without entries", () => {
    const { container } = renderWith(<BasisUpdatesSection entries={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});
