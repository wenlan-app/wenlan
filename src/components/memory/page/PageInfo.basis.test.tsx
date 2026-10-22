// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { BasisRef, Page } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPages: vi.fn(),
  searchPages: vi.fn(),
}));

import { listPages, searchPages } from "../../../lib/tauri";
import PageInfo from "./PageInfo";

const page = (id: string, over: Partial<Page> = {}): Page => ({
  id, title: `Note ${id}`, summary: null, content: "", entity_id: null, domain: null,
  space: "Wenlan", source_memory_ids: [], version: 1, status: "active",
  created_at: "2026-10-08", last_compiled: "2026-10-08", last_modified: "2026-10-08", ...over,
});

const basis = (over: Partial<BasisRef> = {}): BasisRef => ({
  page_id: "basis-a", title: "Pricing", state: "ok", from_version: 1, to_version: 1, ...over,
});

function setup(props: Partial<React.ComponentProps<typeof PageInfo>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onMarkBasis = vi.fn().mockResolvedValue(true);
  const onUnmarkBasis = vi.fn();
  const onPageClick = vi.fn();
  render(<QueryClientProvider client={client}><PageInfo
    embedded
    sourceCount={3}
    sources={[]}
    inbound={[]}
    revisions={[]}
    citations={undefined}
    citationState="none"
    onMemoryClick={vi.fn()}
    onPageClick={onPageClick}
    currentPageId="page-1"
    currentSpace="Wenlan"
    basis={[]}
    onMarkBasis={onMarkBasis}
    onUnmarkBasis={onUnmarkBasis}
    {...props}
  /></QueryClientProvider>);
  return { onMarkBasis, onUnmarkBasis, onPageClick };
}

afterEach(() => {
  cleanup();
  vi.mocked(listPages).mockReset();
  vi.mocked(searchPages).mockReset();
});

describe("PageInfo notes group", () => {
  it("hints that related notes can be sources when there are none", () => {
    setup();
    const group = screen.getByRole("region", { name: "Pages this page relies on" });
    expect(within(group).getByRole("heading", { name: "Pages" })).toBeInTheDocument();
    expect(group).toHaveTextContent("You can also use related pages as sources. You'll be told when they change.");
  });

  it("lists the notes it relies on in plain words and opens or drops them", () => {
    const { onPageClick, onUnmarkBasis } = setup({
      basis: [
        basis(),
        basis({ page_id: "basis-b", title: "Roadmap", state: "pending" }),
        basis({ page_id: "basis-c", title: "Old plan", state: "archived" }),
        basis({ page_id: "basis-d", title: "Elsewhere", state: "unavailable" }),
      ],
    });
    const group = screen.getByRole("region", { name: "Pages this page relies on" });
    expect(group).not.toHaveTextContent("You can also use related pages");
    expect(within(group).getByText("Up to date")).toBeInTheDocument();
    expect(within(group).getByText("Has an update to confirm")).toBeInTheDocument();
    expect(within(group).getByText("Archived")).toBeInTheDocument();
    expect(within(group).getByText("In another space")).toBeInTheDocument();

    fireEvent.click(within(group).getByRole("button", { name: "Roadmap" }));
    expect(onPageClick).toHaveBeenCalledWith("basis-b");

    fireEvent.click(within(group).getAllByRole("button", { name: "More actions" })[0]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop using as a source" }));
    expect(onUnmarkBasis).toHaveBeenCalledWith("basis-a");
  });

  it("picks active notes that are not already sources, own Space first, then other Spaces", async () => {
    vi.mocked(listPages).mockResolvedValue([
      page("page-7", { title: "Other space", space: "Elsewhere" }),
      page("page-1"),
      page("basis-a", { title: "Pricing" }),
      page("page-9", { title: "Eligible" }),
      page("page-8", { title: "Archived one", status: "archived" }),
      page("page-6", { title: "Entity page", creation_kind: "entity" }),
      page("page-5", { title: "In no space", space: null }),
    ]);
    const { onMarkBasis } = setup({ basis: [basis()] });

    fireEvent.click(screen.getByRole("button", { name: "Add a page" }));
    const picker = screen.getByRole("group", { name: "Choose a page to use as a source" });
    expect(await within(picker).findByRole("button", { name: "Eligible" })).toBeInTheDocument();
    expect(listPages).toHaveBeenCalledWith("active", "Wenlan", 40);
    expect(listPages).toHaveBeenCalledWith("active", undefined, 40);
    // A note in another named Space is offered with its Space, after the own Space.
    const names = within(picker).getAllByRole("button").map((button) => button.textContent);
    expect(names).toEqual(["Eligible", "Other spaceElsewhere"]);
    for (const hidden of ["Pricing", "Note page-1", "Archived one", "Entity page", "In no space"]) {
      expect(within(picker).queryByRole("button", { name: hidden })).toBeNull();
    }

    fireEvent.click(within(picker).getByRole("button", { name: "Eligible" }));
    await waitFor(() => expect(onMarkBasis).toHaveBeenCalledWith("page-9"));
    await waitFor(() => expect(screen.queryByRole("group", { name: "Choose a page to use as a source" })).toBeNull());
  });

  it("searches when the person types and keeps the picker open after a failure", async () => {
    vi.mocked(listPages).mockResolvedValue([]);
    vi.mocked(searchPages).mockResolvedValue([page("page-5", { title: "Pricing FAQ" })]);
    const { onMarkBasis } = setup();
    onMarkBasis.mockResolvedValue(false);

    fireEvent.click(screen.getByRole("button", { name: "Add a page" }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search pages" }), { target: { value: "pricing" } });
    fireEvent.click(await screen.findByRole("button", { name: "Pricing FAQ" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not use it as a source. Try again.");
    expect(searchPages).toHaveBeenCalledWith("pricing", 40);
    expect(screen.getByRole("group", { name: "Choose a page to use as a source" })).toBeInTheDocument();
  });

  it("shows the split count in the summary once the note relies on notes", () => {
    setup({ embedded: false, basis: [basis(), basis({ page_id: "basis-b", title: "Roadmap" })] });
    expect(screen.getByText(/3 source memories · 2 pages/)).toBeInTheDocument();
  });

  it("leaves out a zero memory count for a note built only on notes", () => {
    setup({ embedded: false, sourceCount: 0, basis: [basis()] });
    expect(screen.getByText(/0 revisions · 1 page$/)).toBeInTheDocument();
    expect(screen.queryByText(/source memor/)).toBeNull();
  });

  it("leaves the old summary and no notes group for a note with no basis controls", () => {
    setup({ embedded: false, basis: undefined, onMarkBasis: undefined, onUnmarkBasis: undefined });
    expect(screen.getByText(/0 backlinks · 0 revisions · 3 sources/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Pages this page relies on" })).toBeNull();
  });
});
