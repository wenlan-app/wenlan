// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ getSpaceRules: vi.fn() }));
vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getSpaceRules: api.getSpaceRules,
}));

import { NO_SAVED_RULES_PAGES } from "./howIWorkModel";
import { useSavedRulesPageIds } from "./useSavedRulesPages";

function wrapper(client: QueryClient) {
  return ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

afterEach(() => { vi.clearAllMocks(); });

describe("useSavedRulesPageIds", () => {
  // An inline `select` ran on every render and handed back a new Set the
  // query could not share, so the query told every reader its answer had
  // changed, and they re-rendered for ever ("Maximum update depth").
  it.each([
    ["names saved-rules pages", ["page-a", "page-b"]],
    ["names none", []],
  ])("keeps one Set for one daemon answer when it %s", async (_label, ids) => {
    api.getSpaceRules.mockResolvedValue({ saved_rules_page_ids: ids });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const seen: ReadonlySet<string>[] = [];
    const errors = vi.spyOn(console, "error");
    function Reader() {
      const pages = useSavedRulesPageIds();
      seen.push(pages);
      return <span data-testid="count">{pages.size}</span>;
    }
    const { rerender } = render(<Reader />, { wrapper: wrapper(client) });
    await waitFor(() => expect(api.getSpaceRules).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByTestId("count").textContent).toBe(String(ids.length)));
    const answered = seen.length;
    const settled = seen[seen.length - 1];
    rerender(<Reader />);
    rerender(<Reader />);
    expect(seen.length).toBe(answered + 2);
    expect(seen[seen.length - 1]).toBe(settled);
    expect([...settled].sort()).toEqual(ids);
    if (ids.length === 0) expect(settled).toBe(NO_SAVED_RULES_PAGES);
    expect(answered).toBeLessThan(10);
    expect(errors.mock.calls.filter((call) => String(call[0]).includes("Maximum update depth"))).toEqual([]);
    errors.mockRestore();
  });
});
