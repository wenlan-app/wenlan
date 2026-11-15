// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { liveInvoke } from "./live-invoke";
import {
  forgetTeamKey,
  isTeamMemberMode,
  readTeamKey,
  resetTeamMeForTests,
  TEAM_KEY_STORAGE,
  teamCallAllowed,
  TeamCallUnavailable,
} from "./team";
import { teamAccessFor } from "./teamAccess";

function at(path: string) {
  window.history.pushState({}, "", path);
}

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
    expect(fetch).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
  });

  it("sends nothing without a key", async () => {
    at("/app");
    const fetch = vi.fn(async () => new Response("[]"));
    vi.stubGlobal("fetch", fetch);
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(liveInvoke("list_spaces")).rejects.toBeTruthy();
    expect(fetch).not.toHaveBeenCalled();
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
