// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { liveInvoke } from "./live-invoke";

describe("production browser adapter", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("calls the daemon at the same origin", async () => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    const fetch = vi.fn(async () => new Response(JSON.stringify({ background_ai_enabled: true })));
    vi.stubGlobal("fetch", fetch);
    await expect(liveInvoke("get_background_ai_enabled")).resolves.toBe(true);
    expect(fetch).toHaveBeenCalledWith("/api/config", expect.objectContaining({ method: "GET" }));
  });

  it("rejects unmapped and native-only commands instead of returning fixtures", async () => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    await expect(liveInvoke("not_a_real_command")).rejects.toThrow("desktop app");
    await expect(liveInvoke("page_review_supported")).rejects.toThrow("desktop app");
  });
});

describe("pageFromWire", () => {
  it("folds the pages a page relies on into it, as the desktop app does", async () => {
    const { pageFromWire } = await import("./live-invoke");
    const page = pageFromWire({
      page: { id: "p1" },
      basis: [{ page_id: "b1", state: "ok" }],
      code_basis: [{ file: "a.rs" }],
    }) as Record<string, unknown>;
    expect(page.basis).toEqual([{ page_id: "b1", state: "ok" }]);
    expect(page.code_basis).toEqual([{ file: "a.rs" }]);
    expect(pageFromWire({ page: { id: "p2", basis: [] }, basis: [{ page_id: "x" }] })).toEqual({ id: "p2", basis: [] });
    expect(pageFromWire({ page: null })).toBeNull();
    expect(pageFromWire({})).toBeNull();
  });
});
