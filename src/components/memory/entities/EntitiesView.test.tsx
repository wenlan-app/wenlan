// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import {
  archiveEntities,
  confirmEntity,
  deleteEntity,
  queryEntities,
  restoreEntities,
  type Entity,
} from "../../../lib/tauri";
import { EntitiesView } from "./EntitiesView";
import { loadActiveTopicsPage, type TopicCursor } from "./topicBrowse";
import { DEFAULT_FILTERS, type EntityFilters } from "./entitiesViewModel";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("./topicBrowse", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./topicBrowse")>()),
  loadActiveTopicsPage: vi.fn(),
}));
vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  queryEntities: vi.fn(),
  archiveEntities: vi.fn(),
  restoreEntities: vi.fn(),
  confirmEntity: vi.fn(),
  deleteEntity: vi.fn(),
}));

function entity(overrides: Partial<Entity> & { id: string; name: string }): Entity {
  return {
    entity_type: "concept",
    domain: null,
    source_agent: null,
    confidence: null,
    confirmed: false,
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    memory_count: 0,
    status: "detected",
    established_by: null,
    ...overrides,
  };
}

function cursor(offset: number): TopicCursor {
  const stream = { buffer: [], offset, total: 300, exhausted: false };
  return { detected: stream, established: stream };
}

function page(entities: Entity[], hasMore = false, nextCursor = cursor(entities.length)) {
  return { entities, total: entities.length, cursor: nextCursor, hasMore };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

let fixture: Entity[];

beforeEach(() => {
  vi.resetAllMocks();
  window.localStorage.setItem("wenlan-entities-view-mode", "rows");
  fixture = [
    entity({ id: "ada", name: "Ada Lovelace", entity_type: "person", space: "History" }),
    entity({ id: "engine", name: "Analytical Engine", status: "established", confirmed: true, memory_count: 5 }),
    entity({ id: "countess", name: "Countess of Lovelace", entity_type: "person", status: "archived" }),
  ];
  vi.mocked(loadActiveTopicsPage).mockImplementation(async (filters: EntityFilters, nextCursor?: TopicCursor) => {
    const all = fixture.filter((candidate) =>
      candidate.status !== "archived"
      && (filters.type === "all" || candidate.entity_type === filters.type)
      && candidate.name.toLowerCase().includes(filters.query.trim().toLowerCase()),
    );
    const offset = nextCursor?.detected.offset ?? 0;
    return { entities: all.slice(offset, offset + 100), total: all.length, cursor: cursor(offset + 100), hasMore: offset + 100 < all.length };
  });
  vi.mocked(queryEntities).mockImplementation(async (filter) => {
    const all = fixture.filter((candidate) =>
      candidate.status === filter.status
      && (!filter.entity_type || candidate.entity_type === filter.entity_type)
      && (!filter.query || candidate.name.toLowerCase().includes(filter.query.toLowerCase())),
    );
    const offset = filter.offset ?? 0;
    return { entities: all.slice(offset, offset + (filter.limit ?? 100)), total: all.length };
  });
});

afterEach(() => cleanup());

function renderView() {
  const onEntityClick = vi.fn();
  return { onEntityClick, user: userEvent.setup(), ...render(<EntitiesView onEntityClick={onEntityClick} />) };
}

async function openArchived(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Topic options" }));
  await user.click(screen.getByRole("menuitem", { name: "View archived topics" }));
}

function expectNoMutations() {
  for (const mutation of [archiveEntities, confirmEntity, deleteEntity, restoreEntities]) {
    expect(mutation).not.toHaveBeenCalled();
  }
}

describe("EntitiesView browse", () => {
  it("browses detected and established topics together without lifecycle controls or counts", async () => {
    const { user, onEntityClick } = renderView();
    await screen.findByRole("button", { name: "Ada Lovelace" });
    expect(screen.getByRole("button", { name: "Analytical Engine" })).toBeInTheDocument();
    expect(screen.queryByText("Countess of Lovelace")).not.toBeInTheDocument();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Confirm|Archive all|Restore|Delete/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("columnheader")).not.toBeInTheDocument();
    expect(within(screen.getByTestId("entities-list")).getByText("Person")).toBeInTheDocument();
    expect(screen.queryByText(/5 memories|Confirmed by|Detected in/)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Ada Lovelace" }));
    await user.click(screen.getByRole("button", { name: "Analytical Engine" }));
    expect(onEntityClick.mock.calls).toEqual([["ada"], ["engine"]]);
    expect(queryEntities).not.toHaveBeenCalled();
    expectNoMutations();
  });

  it("uses the same openable, quiet cards for every topic status and retains the lens through archive navigation", async () => {
    window.localStorage.removeItem("wenlan-entities-view-mode");
    const { user, onEntityClick } = renderView();
    const card = await screen.findByTestId("entity-card-ada");
    expect(within(card).getByText("Person")).toBeInTheDocument();
    expect(within(card).getByText("History")).toBeInTheDocument();
    expect(card).toHaveAccessibleName("Ada Lovelace");
    expect(within(card).queryByRole("button")).not.toBeInTheDocument();
    const detectedOpen = screen.getByRole("button", { name: "Ada Lovelace" });
    detectedOpen.focus();
    await user.keyboard("{Enter}");
    await user.click(screen.getByRole("button", { name: "Analytical Engine" }));
    await openArchived(user);
    await user.click(screen.getByRole("button", { name: "Countess of Lovelace" }));
    expect(screen.getByTestId("asset-lens-cards")).toHaveAttribute("aria-pressed", "true");
    expect(onEntityClick.mock.calls).toEqual([["ada"], ["engine"], ["countess"]]);
    expectNoMutations();
  });

  it("keeps archived topics behind options, opens their details and returns to active topics", async () => {
    const { user, onEntityClick } = renderView();
    await screen.findByText("Ada Lovelace");
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "View archived topics" })).not.toBeInTheDocument();
    await openArchived(user);
    expect(await screen.findByRole("heading", { name: "Archived topics", level: 1 })).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Countess of Lovelace" }));
    expect(onEntityClick).toHaveBeenCalledWith("countess");
    expect(queryEntities).toHaveBeenLastCalledWith({ status: "archived", limit: 100, offset: 0 });
    expect(screen.queryByRole("button", { name: /Restore|Delete|Confirm/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Back to topics" }));
    await screen.findByText("Analytical Engine");
    expect(screen.getByRole("heading", { name: "Topics", level: 1 })).toBeInTheDocument();
    expect(screen.queryByText("Countess of Lovelace")).not.toBeInTheDocument();
    expectNoMutations();
  });

  it("focuses the archive option and returns focus to the options button on Escape", async () => {
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    const options = screen.getByRole("button", { name: "Topic options" });
    await user.click(options);
    expect(screen.getByRole("menuitem", { name: "View archived topics" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    expect(options).toHaveFocus();
  });

  it("filters both active lifecycles with familiar type chips and preserves filters in archived view", async () => {
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    await user.click(screen.getByRole("button", { name: "Concept" }));
    await waitFor(() => expect(screen.queryByText("Ada Lovelace")).not.toBeInTheDocument());
    expect(screen.getByText("Analytical Engine")).toBeInTheDocument();
    await openArchived(user);
    expect(await screen.findByText("No matching topics")).toBeInTheDocument();
    expect(queryEntities).toHaveBeenLastCalledWith({ status: "archived", limit: 100, offset: 0, entity_type: "concept" });
    await user.click(screen.getByRole("button", { name: "Person" }));
    await screen.findByText("Countess of Lovelace");
  });

  it("exposes every canonical editable topic type as a browse filter", async () => {
    fixture = [
      entity({ id: "technology", name: "TypeScript", entity_type: "technology", status: "established" }),
      entity({ id: "project", name: "Wenlan", entity_type: "project", status: "established" }),
    ];
    const { user } = renderView();
    await screen.findByRole("button", { name: "TypeScript" });

    await user.click(screen.getByRole("button", { name: "Technology" }));

    expect(await screen.findByRole("button", { name: "TypeScript" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Wenlan" })).not.toBeInTheDocument();
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith({ query: "", type: "technology", memories: "any" }, undefined);
    expect(screen.getByRole("button", { name: "Project" })).toBeInTheDocument();
  });

  it("debounces search and distinguishes a filtered no-match from an empty library", async () => {
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    await user.type(screen.getByRole("searchbox", { name: "Filter topics" }), "unmatched");
    expect(loadActiveTopicsPage).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("No matching topics")).toBeInTheDocument();
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith({ query: "unmatched", type: "all", memories: "any" }, undefined);
    fixture = [];
    await user.clear(screen.getByRole("searchbox", { name: "Filter topics" }));
    expect(await screen.findByText("No topics yet")).toBeInTheDocument();
  });

  it("switches the lens without refetching and keeps row names keyboard accessible", async () => {
    const { user, onEntityClick } = renderView();
    await screen.findByText("Ada Lovelace");
    await user.click(screen.getByTestId("asset-lens-cards"));
    expect(screen.getByTestId("entities-cards")).toBeInTheDocument();
    await user.click(screen.getByTestId("asset-lens-rows"));
    const name = screen.getByRole("button", { name: "Ada Lovelace" });
    name.focus();
    await user.keyboard(" ");
    expect(onEntityClick).toHaveBeenCalledWith("ada");
    expect(window.localStorage.getItem("wenlan-entities-view-mode")).toBe("rows");
    expect(loadActiveTopicsPage).toHaveBeenCalledTimes(1);
  });

  it("appends pages using the returned cursor, prevents duplicate load more, and retains all rows", async () => {
    const next = deferred<ReturnType<typeof page>>();
    const firstCursor = cursor(100);
    vi.mocked(loadActiveTopicsPage)
      .mockResolvedValueOnce(page([fixture[0]], true, firstCursor))
      .mockImplementationOnce(() => next.promise);
    renderView();
    await screen.findByText("Ada Lovelace");
    const more = screen.getByRole("button", { name: "Load more" });
    fireEvent.click(more);
    fireEvent.click(more);
    expect(more).toBeDisabled();
    expect(loadActiveTopicsPage).toHaveBeenCalledTimes(2);
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith({ query: "", type: "all", memories: "any" }, firstCursor);
    await act(async () => next.resolve(page([fixture[1]], false)));
    expect(screen.getByText("Ada Lovelace")).toBeInTheDocument();
    expect(screen.getByText("Analytical Engine")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();
  });

  it("retains rows and the same cursor after a load-more failure so retry loses nothing", async () => {
    const firstCursor = cursor(100);
    vi.mocked(loadActiveTopicsPage)
      .mockResolvedValueOnce(page([fixture[0]], true, firstCursor))
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce(page([fixture[1]]));
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("Could not load items. Try again."));
    expect(screen.getByText("Ada Lovelace")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await screen.findByText("Analytical Engine");
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith({ query: "", type: "all", memories: "any" }, firstCursor);
    expect(screen.getByText("Ada Lovelace")).toBeInTheDocument();
  });

  // CPU-bound: 100+ archived rows are each a button, so a button role query
  // computes an accessible name for every row. Those two queries took most of
  // this test's time and it hit the 5 s timeout on a loaded CI runner. Plain
  // text queries scoped to buttons find the same controls without that scan.
  it("paginates archived topics with offsets and resets the active cursor when returning", async () => {
    fixture.push(...Array.from({ length: 101 }, (_, index) => entity({ id: `archive-${index}`, name: `Archived ${index}`, status: "archived" })));
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    await openArchived(user);
    await screen.findByText("Countess of Lovelace");
    await user.click(screen.getByText("Load more", { selector: "button" }));
    await screen.findByText("Archived 100");
    expect(queryEntities).toHaveBeenLastCalledWith({ status: "archived", limit: 100, offset: 100 });
    expect(screen.getByText("Countess of Lovelace")).toBeInTheDocument();
    await user.click(screen.getByText("Back to topics", { selector: "button" }));
    await screen.findByText("Ada Lovelace");
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith({ query: "", type: "all", memories: "any" }, undefined);
  });

  it("ignores stale active responses after switching to archived topics", async () => {
    const slow = deferred<ReturnType<typeof page>>();
    vi.mocked(loadActiveTopicsPage).mockImplementationOnce(() => slow.promise);
    const { user } = renderView();
    await openArchived(user);
    await screen.findByText("Countess of Lovelace");
    await act(async () => slow.resolve(page([fixture[0]], true)));
    expect(screen.queryByText("Ada Lovelace")).not.toBeInTheDocument();
    expect(screen.getByText("Countess of Lovelace")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();
  });

  it("ignores a stale search response and uses a fresh cursor for the newest search", async () => {
    const slow = deferred<ReturnType<typeof page>>();
    vi.mocked(loadActiveTopicsPage)
      .mockResolvedValueOnce(page(fixture.slice(0, 2)))
      .mockImplementationOnce(() => slow.promise)
      .mockResolvedValueOnce(page([fixture[1]]));
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    const search = screen.getByRole("searchbox", { name: "Filter topics" });
    await user.type(search, "Ada");
    await waitFor(() => expect(loadActiveTopicsPage).toHaveBeenCalledTimes(2));
    await user.clear(search);
    await user.type(search, "Engine");
    await screen.findByText("Analytical Engine");
    await act(async () => slow.resolve(page([fixture[0]], true)));
    expect(screen.queryByText("Ada Lovelace")).not.toBeInTheDocument();
    expect(screen.getByText("Analytical Engine")).toBeInTheDocument();
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith({ query: "Engine", type: "all", memories: "any" }, undefined);
  });

  it("ignores stale load-more responses and failures after a type filter changes", async () => {
    const slow = deferred<ReturnType<typeof page>>();
    vi.mocked(loadActiveTopicsPage)
      .mockResolvedValueOnce(page([fixture[0]], true))
      .mockImplementationOnce(() => slow.promise)
      .mockResolvedValueOnce(page([fixture[1]]));
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await user.click(screen.getByRole("button", { name: "Concept" }));
    await screen.findByText("Analytical Engine");
    await act(async () => slow.reject(new Error("old request failed")));
    expect(screen.queryByText("Ada Lovelace")).not.toBeInTheDocument();
    expect(screen.getByText("Analytical Engine")).toBeInTheDocument();
    expect(toast.error).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows a load error and retries without misreporting an empty library", async () => {
    vi.mocked(loadActiveTopicsPage).mockRejectedValueOnce(new Error("offline"));
    const { user } = renderView();
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load items. Try again.");
    expect(screen.queryByText("No topics yet")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByText("Ada Lovelace");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("invalidates requests on unmount, including load-more failure notifications", async () => {
    const slow = deferred<ReturnType<typeof page>>();
    vi.mocked(loadActiveTopicsPage)
      .mockResolvedValueOnce(page([fixture[0]], true))
      .mockImplementationOnce(() => slow.promise);
    const { user, unmount } = renderView();
    await screen.findByText("Ada Lovelace");
    await user.click(screen.getByRole("button", { name: "Load more" }));
    unmount();
    await act(async () => slow.reject(new Error("late failure")));
    expect(toast.error).not.toHaveBeenCalled();
  });
});

describe("EntitiesView duplicate response handling", () => {
  it("renders a single freshest row for duplicate IDs in a reset response", async () => {
    const stale = entity({ id: "crossing", name: "Stale snapshot", updated_at: 10 });
    const detected = entity({ id: "crossing", name: "Detected snapshot", updated_at: 20 });
    const established = entity({ id: "crossing", name: "Established snapshot", updated_at: 20, status: "established", confirmed: true });
    vi.mocked(loadActiveTopicsPage).mockResolvedValueOnce(page([stale, established, detected, established]));
    renderView();
    expect(await screen.findByRole("button", { name: "Established snapshot" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Established snapshot" })).toHaveLength(1);
    expect(screen.queryByText("Stale snapshot")).not.toBeInTheDocument();
    expect(screen.queryByText("Detected snapshot")).not.toBeInTheDocument();
    expectNoMutations();
  });

  it("updates an existing row and deduplicates each appended batch while retaining its cursor", async () => {
    const old = entity({ id: "crossing", name: "Old topic", updated_at: 10 });
    const established = entity({ id: "crossing", name: "Established latest", updated_at: 20, status: "established", confirmed: true });
    const other = entity({ id: "other", name: "Other latest", updated_at: 30 });
    const firstCursor = cursor(100);
    const secondCursor = cursor(200);
    vi.mocked(loadActiveTopicsPage)
      .mockResolvedValueOnce(page([old], true, firstCursor))
      .mockResolvedValueOnce(page([
        established, { ...other, name: "Other stale", updated_at: 5 }, other, established,
      ], true, secondCursor))
      .mockResolvedValueOnce(page([
        { ...old, name: "Tie detected", updated_at: 20 }, old, { ...other, updated_at: 29 },
      ]));
    const { user } = renderView();
    await screen.findByText("Old topic");
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await screen.findByText("Established latest");
    expect(screen.getAllByRole("button", { name: "Established latest" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Other latest" })).toHaveLength(1);
    expect(screen.queryByText("Old topic")).not.toBeInTheDocument();
    expect(screen.queryByText("Other stale")).not.toBeInTheDocument();
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith(DEFAULT_FILTERS, firstCursor);
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument());
    expect(screen.getAllByRole("button", { name: "Established latest" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Other latest" })).toHaveLength(1);
    expect(screen.queryByText("Tie detected")).not.toBeInTheDocument();
    expect(loadActiveTopicsPage).toHaveBeenLastCalledWith(DEFAULT_FILTERS, secondCursor);
    expectNoMutations();
  });

  it("deduplicates archived reset rows while paginating by raw response length", async () => {
    const old = entity({ id: "archive", name: "Old archived", status: "archived", updated_at: 10 });
    const fresh = { ...old, name: "Fresh archived", updated_at: 20 };
    vi.mocked(queryEntities)
      .mockResolvedValueOnce({ entities: [old, fresh], total: 3 })
      .mockResolvedValueOnce({ entities: [entity({ id: "archive-next", name: "Next archived", status: "archived" })], total: 3 });
    const { user } = renderView();
    await screen.findByText("Ada Lovelace");
    await openArchived(user);
    expect(await screen.findByRole("button", { name: "Fresh archived" })).toBeInTheDocument();
    expect(screen.queryByText("Old archived")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await screen.findByText("Next archived");
    expect(queryEntities).toHaveBeenLastCalledWith({ status: "archived", limit: 100, offset: 2 });
    expect(screen.getAllByRole("button", { name: "Fresh archived" })).toHaveLength(1);
    expectNoMutations();
  });
});
