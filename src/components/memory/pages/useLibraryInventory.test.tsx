// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listPagesExplicitBrowse, type Page } from "../../../lib/tauri";
import { useLibraryInventory } from "./useLibraryInventory";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPagesExplicitBrowse: vi.fn(),
}));

function note(status: "active" | "draft"): Page {
  return {
    id: `${status}-1`,
    title: `A ${status} note`,
    summary: "",
    content: "",
    entity_id: null,
    domain: null,
    space: null,
    source_memory_ids: [],
    version: 1,
    status,
    created_at: "2026-07-01T00:00:00Z",
    last_compiled: "2026-07-01T00:00:00Z",
    last_modified: "2026-07-10T00:00:00Z",
  };
}

function renderInventory() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { client, ...renderHook(() => useLibraryInventory(), { wrapper }) };
}

describe("useLibraryInventory", () => {
  beforeEach(() => vi.mocked(listPagesExplicitBrowse).mockReset());

  it("is not empty while loading, then empty once both lists load with no notes", async () => {
    vi.mocked(listPagesExplicitBrowse).mockResolvedValue([]);
    const { result } = renderInventory();
    expect(result.current).toMatchObject({ ready: false, empty: false });
    await waitFor(() => expect(result.current.empty).toBe(true));
    expect(result.current.ready).toBe(true);
  });

  it.each(["active", "draft"] as const)("is not empty once a %s note exists", async status => {
    vi.mocked(listPagesExplicitBrowse).mockImplementation(async requested => requested === status ? [note(status)] : []);
    const { result } = renderInventory();
    await waitFor(() => expect(result.current.ready).toBe(true));
    expect(result.current.empty).toBe(false);
    expect(result.current.pages).toHaveLength(1);
  });

  it("returns to non-empty as soon as the cached list gains a note", async () => {
    vi.mocked(listPagesExplicitBrowse).mockResolvedValue([]);
    const { result, client } = renderInventory();
    await waitFor(() => expect(result.current.empty).toBe(true));
    client.setQueryData(["pages", "draft"], [note("draft")]);
    await waitFor(() => expect(result.current.empty).toBe(false));
  });

  it("is not empty while either list is still loading", async () => {
    vi.mocked(listPagesExplicitBrowse).mockImplementation(status => status === "draft"
      ? new Promise<Page[]>(() => {})
      : Promise.resolve([]));
    const { result, client } = renderInventory();
    await waitFor(() => expect(client.getQueryState(["pages", "active"])?.status).toBe("success"));
    expect(result.current).toMatchObject({ ready: false, empty: false });
  });
});
