// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { liveInvoke } from "./live-invoke";
import {
  forgetTeamKey,
  isTeamMemberMode,
  linkProvider,
  readTeamKey,
  resetTeamMeForTests,
  signOutBrowser,
  signOutEverywhere,
  TEAM_KEY_STORAGE,
  teamCallAllowed,
  TeamCallUnavailable,
  teamMe,
  teamProviders,
} from "./team";
import { teamAccessFor } from "./teamAccess";

function at(path: string) {
  window.history.pushState({}, "", path);
}

const me = (signedInWith?: "browser" | "key") => ({ member_id: "m1", name: "Dana", spaces: [], signed_in_with: signedInWith });
const asked = (fetch: ReturnType<typeof vi.fn>) => (fetch.mock.calls as unknown as [string, RequestInit][]).map(([url, init]) => ({ url, init }));

describe("team-member mode", () => {
  beforeEach(() => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.localStorage.clear();
    resetTeamMeForTests();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.localStorage.clear();
    at("/");
  });

  it("switches on only for the web bundle at /app", () => {
    at("/app");
    expect(isTeamMemberMode()).toBe(true);
    at("/app/pages/x");
    expect(isTeamMemberMode()).toBe(true);
    at("/application");
    expect(isTeamMemberMode()).toBe(false);
    at("/");
    expect(isTeamMemberMode()).toBe(false);
    at("/app");
    vi.stubGlobal("__WENLAN_WEB__", false);
    expect(isTeamMemberMode()).toBe(false);
  });

  it("allows exactly the gate's rows", () => {
    expect(teamCallAllowed("GET", "/api/spaces")).toBe(true);
    expect(teamCallAllowed("GET", "/api/pages?space=Field%20Notes")).toBe(true);
    expect(teamCallAllowed("GET", "/api/pages/page_1")).toBe(true);
    expect(teamCallAllowed("GET", "/api/pages/recent")).toBe(true);
    expect(teamCallAllowed("POST", "/api/pages/drafts")).toBe(true);
    expect(teamCallAllowed("PUT", "/api/pages/drafts/d1")).toBe(true);
    // A literal segment is never read as a page id.
    expect(teamCallAllowed("GET", "/api/pages/orphan-links")).toBe(false);
    expect(teamCallAllowed("DELETE", "/api/pages/page_1")).toBe(false);
    expect(teamCallAllowed("GET", "/api/memory/graph")).toBe(true);
    expect(teamCallAllowed("POST", "/api/pages/page_1/archive")).toBe(true);
    expect(teamCallAllowed("POST", "/api/pages/page_1/restore")).toBe(true);
    expect(teamCallAllowed("PUT", "/api/pages/page_1/basis/page_2")).toBe(true);
    expect(teamCallAllowed("DELETE", "/api/pages/page_1/basis/page_2")).toBe(true);
    expect(teamCallAllowed("DELETE", "/api/pages/page_1/basis/")).toBe(false);
    // Deleting a page for good, and locking it, stay the owner's.
    expect(teamCallAllowed("DELETE", "/api/pages/page_1?permanent=true")).toBe(false);
    expect(teamCallAllowed("POST", "/api/pages/page_1/lock")).toBe(false);
    expect(teamCallAllowed("GET", "/api/config")).toBe(false);
    expect(teamCallAllowed("GET", "/api/memories")).toBe(false);
    expect(teamCallAllowed("GET", "/api/pages/../config")).toBe(false);
  });

  it("sends gate calls to the team prefix with the member's key and no cookies", async () => {
    at("/app");
    window.localStorage.setItem(TEAM_KEY_STORAGE, "  key-123 ");
    const fetch = vi.fn(async () => new Response("[]"));
    vi.stubGlobal("fetch", fetch);
    await liveInvoke("list_spaces");
    expect(fetch).toHaveBeenCalledWith(
      "/team/v1/app/api/spaces",
      expect.objectContaining({
        method: "GET",
        credentials: "omit",
        headers: expect.objectContaining({ Authorization: "Bearer key-123" }),
      }),
    );
  });

  it("asks for one Space's recent changes in the query, never a header", async () => {
    at("/app");
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    const fetch = vi.fn(async () => new Response("[]"));
    vi.stubGlobal("fetch", fetch);
    await liveInvoke("list_recent_changes", { limit: 5, space: "產品" });
    expect(fetch).toHaveBeenCalledWith(
      `/team/v1/app/api/pages/recent-changes?limit=5&space=${encodeURIComponent("產品")}`,
      expect.objectContaining({ headers: { Authorization: "Bearer key-123" } }),
    );
  });

  it("answers calls with no gate row without a request or a warning", async () => {
    at("/app");
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    const fetch = vi.fn(async () => new Response("{}"));
    vi.stubGlobal("fetch", fetch);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(liveInvoke("get_background_ai_enabled")).rejects.toBeInstanceOf(TeamCallUnavailable);
    await expect(liveInvoke("search", { query: "x" })).resolves.toEqual([]);
    await expect(liveInvoke("search_conversations", { query: "x" })).resolves.toEqual([]);
    await expect(liveInvoke("list_shares")).resolves.toEqual([]);
    await expect(liveInvoke("get_page_links", { pageId: "page_x" })).resolves.toEqual({ outbound: [], inbound: [] });
    expect(fetch).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
  });

  it("sends the browser session, and no Authorization header, when there is no key", async () => {
    at("/app");
    const fetch = vi.fn(async () => new Response("[]"));
    vi.stubGlobal("fetch", fetch);
    await liveInvoke("list_spaces");
    expect(fetch).toHaveBeenCalledTimes(1);
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/team/v1/app/api/spaces");
    expect(init.credentials).toBe("same-origin");
    expect(init.headers).not.toHaveProperty("Authorization");
  });

  it("prefers the session once teamMe has seen it work, even if a key were still stored", async () => {
    at("/app");
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    const calls: RequestInit[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      calls.push(init);
      return url === "/team/v1/me" ? new Response(JSON.stringify(me("browser"))) : new Response("[]");
    }));
    await teamMe();
    await liveInvoke("list_spaces");
    expect(calls[calls.length - 1].credentials).toBe("same-origin");
    expect(calls[calls.length - 1].headers).not.toHaveProperty("Authorization");
  });

  it("asks who this is again after the session ends (a 401 on the gate)", async () => {
    at("/app");
    const fetch = vi.fn(async (url: string) => (url === "/team/v1/me" ? new Response(JSON.stringify(me("browser"))) : new Response("", { status: 401 })));
    vi.stubGlobal("fetch", fetch);
    await teamMe();
    await expect(liveInvoke("list_spaces")).rejects.toBeTruthy();
    await teamMe();
    expect(fetch.mock.calls.filter(([url]) => url === "/team/v1/me")).toHaveLength(2);
  });

  it("forgets the key in this browser", () => {
    at("/app");
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    const assign = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign } as Location);
    forgetTeamKey();
    expect(window.localStorage.getItem(TEAM_KEY_STORAGE)).toBeNull();
    expect(readTeamKey()).toBeNull();
    expect(assign).toHaveBeenCalledWith("/app");
  });

  it("gives edit access only in write Spaces", () => {
    const access = teamAccessFor({
      member_id: "m1",
      name: "Mira",
      spaces: [
        { space: "Field Notes", access: "write" },
        { space: "Archive", access: "read" },
      ],
    });
    expect(access.member).toBe(true);
    expect(access.canEdit("Field Notes")).toBe(true);
    expect(access.canEdit("Archive")).toBe(false);
    expect(access.canEdit("Kitchen")).toBe(false);
    expect(access.canEdit(null)).toBe(false);
    expect(access.writable).toEqual(["Field Notes"]);
  });
});

describe("who this browser is", () => {
  beforeEach(() => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.localStorage.clear();
    resetTeamMeForTests();
    at("/app");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.localStorage.clear();
    at("/");
  });

  it("tries the browser session when there is no key, and a 401 means signed out", async () => {
    const fetch = vi.fn(async () => new Response("", { status: 401 }));
    vi.stubGlobal("fetch", fetch);
    expect(await teamMe()).toBeNull();
    const [only] = asked(fetch);
    expect(only.url).toBe("/team/v1/me");
    expect(only.init.credentials).toBe("same-origin");
    expect(only.init.headers).toBeUndefined();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("answers from the session and drops a stored key only after the session works", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    let seen = 0;
    vi.stubGlobal("fetch", vi.fn(async () => {
      // The key is still stored while the session is being asked.
      seen += window.localStorage.getItem(TEAM_KEY_STORAGE) ? 1 : 0;
      return new Response(JSON.stringify({ ...me("browser"), accounts: [{ provider: "google", email: "dana@gmail.com" }] }));
    }));
    const found = await teamMe();
    expect(found?.signed_in_with).toBe("browser");
    expect(found?.accounts?.[0].email).toBe("dana@gmail.com");
    expect(seen).toBe(1);
    expect(readTeamKey()).toBeNull();
  });

  it("keeps the key when the session does not work, and uses it", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    const fetch = vi.fn(async (_url: string, init: RequestInit) => (
      (init.headers as Record<string, string> | undefined)?.Authorization === "Bearer key-123"
        ? new Response(JSON.stringify(me()))
        : new Response("", { status: 401 })
    ));
    vi.stubGlobal("fetch", fetch);
    const found = await teamMe();
    expect(found?.signed_in_with).toBe("key");
    expect(readTeamKey()).toBe("key-123");
    const [first, second] = asked(fetch);
    expect(first.init.credentials).toBe("same-origin");
    expect(second.init.credentials).toBe("omit");
  });

  it("keeps the key when the server is unreachable, so nothing is lost to a bad network", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("offline"); }));
    expect(await teamMe()).toBeNull();
    expect(readTeamKey()).toBe("key-123");
  });

  it("caches the answer until it is reset by signing out", async () => {
    const fetch = vi.fn(async (..._a: unknown[]) => new Response(JSON.stringify(me("browser"))));
    vi.stubGlobal("fetch", fetch);
    await teamMe();
    await teamMe();
    expect(fetch).toHaveBeenCalledTimes(1);
    const assign = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign } as Location);
    await signOutBrowser();
    expect(asked(fetch)[asked(fetch).length - 1]).toMatchObject({ url: "/auth/logout", init: { method: "POST", credentials: "same-origin" } });
    expect(assign).toHaveBeenCalledWith("/app");
    await teamMe();
    expect(fetch.mock.calls.filter(([url]) => url === "/team/v1/me")).toHaveLength(2);
  });

  it("signs out everywhere with a POST to its own route, then reloads /app", async () => {
    const fetch = vi.fn(async () => new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetch);
    const assign = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign } as Location);
    await signOutEverywhere();
    expect(asked(fetch)[0]).toMatchObject({ url: "/auth/logout-all", init: { method: "POST", credentials: "same-origin" } });
    expect(assign).toHaveBeenCalledWith("/app");
  });

  it("reads which providers are on, and none when sign-in is off or the server is older", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ providers: ["google", "other", "github"], team: "Acme" }))));
    expect(await teamProviders()).toEqual({ providers: ["google", "github"], team: "Acme" });
    resetTeamMeForTests();
    vi.stubGlobal("fetch", vi.fn(async () => new Response("", { status: 404 })));
    expect(await teamProviders()).toEqual({ providers: [], team: null });
  });

  it("starts linking with the key and follows the address the server gives", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    const fetch = vi.fn(async () => new Response(JSON.stringify({ url: "https://accounts.example.com/auth?x=1" })));
    vi.stubGlobal("fetch", fetch);
    const assign = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign } as Location);
    expect(await linkProvider("github")).toBe(true);
    expect(asked(fetch)[0]).toMatchObject({
      url: "/auth/link/github",
      init: { method: "POST", headers: { Authorization: "Bearer key-123" } },
    });
    expect(assign).toHaveBeenCalledWith("https://accounts.example.com/auth?x=1");
    // The key stays until the session it leads to has worked.
    expect(readTeamKey()).toBe("key-123");
  });

  it("says so when the server will not start linking", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-123");
    vi.stubGlobal("fetch", vi.fn(async () => new Response("", { status: 404 })));
    const assign = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign } as Location);
    expect(await linkProvider("google")).toBe(false);
    expect(assign).not.toHaveBeenCalled();
  });
});
