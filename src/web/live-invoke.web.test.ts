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
