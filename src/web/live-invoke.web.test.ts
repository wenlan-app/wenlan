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

describe("person key (UX-SKILL-13)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
    document.head.innerHTML = "";
    window.history.replaceState(null, "", "/");
  });
  const KEY = "ab".repeat(32);

  it("ux_skill_13 web page path: takes the session token from the page once and drops the pass", async () => {
    const { takePersonKeyFromPage, readPersonKey, PERSON_SESSION_META } = await import("./live-invoke");
    window.history.replaceState(null, "", "/?pass=" + "cd".repeat(32) + "&x=1");
    const meta = document.createElement("meta");
    meta.name = PERSON_SESSION_META;
    meta.content = KEY;
    document.head.appendChild(meta);
    takePersonKeyFromPage();
    expect(readPersonKey()).toBe(KEY);
    expect(document.querySelector(`meta[name="${PERSON_SESSION_META}"]`)).toBeNull();
    expect(window.location.search).toBe("?x=1");
  });

  it("ux_skill_13 a page with no key keeps none", async () => {
    const { takePersonKeyFromPage, readPersonKey } = await import("./live-invoke");
    takePersonKeyFromPage();
    expect(readPersonKey()).toBeNull();
  });

  it("ux_skill_13 sends the key only on the person's own gestures", async () => {
    const { liveInvoke, PERSON_KEY_HEADER } = await import("./live-invoke");
    vi.stubGlobal("__WENLAN_WEB__", true);
    const fetch = vi.fn(async () => new Response("{}"));
    vi.stubGlobal("fetch", fetch);
    sessionStorage.setItem("wenlan-person-key", KEY);
    await liveInvoke("accept_note_proposal", { id: "n1" });
    await liveInvoke("get_note_proposal", { id: "n1" });
    const [gesture, read] = fetch.mock.calls as unknown as [string, RequestInit][];
    expect(gesture[1].headers).toMatchObject({ "x-wenlan-caller": "person", [PERSON_KEY_HEADER]: KEY });
    expect(read[1].headers ?? {}).not.toHaveProperty(PERSON_KEY_HEADER);
  });

  it("ux_skill_13 without a key the gesture goes out with the marker only", async () => {
    const { liveInvoke, PERSON_KEY_HEADER } = await import("./live-invoke");
    vi.stubGlobal("__WENLAN_WEB__", true);
    const fetch = vi.fn(async () => new Response("{}"));
    vi.stubGlobal("fetch", fetch);
    await liveInvoke("accept_note_proposal", { id: "n1" });
    const [gesture] = fetch.mock.calls as unknown as [string, RequestInit][];
    expect(gesture[1].headers ?? {}).not.toHaveProperty(PERSON_KEY_HEADER);
  });

  it("does not cache joined team library reads", async () => {
    const { liveInvoke } = await import("./live-invoke");
    vi.stubGlobal("__WENLAN_WEB__", true);
    const fetch = vi.fn(async () => new Response(JSON.stringify({ teams: [] })));
    vi.stubGlobal("fetch", fetch);
    await liveInvoke("team_joined_library");
    await liveInvoke("team_joined_page", { teamId: "t1", pageId: "p1" });
    const calls = fetch.mock.calls as unknown as [string, RequestInit][];
    expect(calls[0]?.[1]).toMatchObject({ cache: "no-store" });
    expect(calls[1]?.[1]).toMatchObject({ cache: "no-store" });
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
