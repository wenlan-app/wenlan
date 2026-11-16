// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { liveInvoke } from "./live-invoke";
import {
  createMemberTheme, getMemberActiveTheme, getMemberTheme, listMemberThemes, resetMemberThemesForTests,
  saveMemberTheme, setMemberActiveTheme,
} from "./memberThemes";
import { resetTeamMeForTests } from "./team";

beforeEach(() => {
  window.localStorage.clear();
  resetMemberThemesForTests();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  window.history.pushState({}, "", "/");
  resetTeamMeForTests();
});

describe("member themes store", () => {
  it("is empty with no storage", () => {
    const list = listMemberThemes();
    expect(list.themes).toEqual([]);
    expect(list.active).toBeUndefined();
    expect(getMemberActiveTheme()).toEqual({ revision: "builtin", tokens: {}, problems: [] });
  });

  it("creates, saves, activates, and lists again after a reload", () => {
    const created = createMemberTheme("Night Bunny", "dark");
    if (created.status !== "ok") throw new Error("refused");
    expect(created.value.id).toBe("night-bunny");
    const saved = saveMemberTheme("night-bunny", { ...created.value.theme, colors: { ...created.value.theme.colors, accent: "#E58BC0" } });
    expect(saved.status).toBe("ok");
    const active = setMemberActiveTheme("night-bunny");
    if (active.status !== "ok") throw new Error("refused");
    expect(active.value.tokens["--mem-accent-indigo"]).toBe("#E58BC0");

    resetMemberThemesForTests(); // a reload keeps only what storage holds
    const list = listMemberThemes();
    expect(list.active).toBe("night-bunny");
    expect(list.themes.map((t) => t.id)).toEqual(["night-bunny"]);
    expect(getMemberTheme("night-bunny").theme.colors.accent).toBe("#E58BC0");
    expect(createMemberTheme("Night Bunny", "light").status === "ok" && listMemberThemes().themes.length).toBe(2);
    expect(setMemberActiveTheme(null).status).toBe("ok");
    expect(listMemberThemes().active).toBeUndefined();
  });

  it("refuses a theme that fails its checks", () => {
    const created = createMemberTheme("Low", "dark");
    if (created.status !== "ok") throw new Error("refused");
    const bad = saveMemberTheme("low", { ...created.value.theme, colors: { ...created.value.theme.colors, text: "#191C24" } });
    expect(bad.status).toBe("refused");
  });

  it("still works when localStorage is absent or throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(listMemberThemes().themes).toEqual([]);
    expect(createMemberTheme("Kept", "light").status).toBe("ok");
    expect(listMemberThemes().themes.map((t) => t.id)).toEqual(["kept"]);
  });

  it("treats broken stored JSON as empty", () => {
    window.localStorage.setItem("wenlan-member-themes", "{nope");
    expect(listMemberThemes().themes).toEqual([]);
  });
});

describe("member mode theme calls", () => {
  it("never fetch /api/themes", async () => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.history.pushState({}, "", "/app");
    resetTeamMeForTests();
    const fetch = vi.fn(async () => new Response("{}"));
    vi.stubGlobal("fetch", fetch);
    await liveInvoke("list_themes");
    const made: any = await liveInvoke("create_theme", { name: "Mine", base: "dark" });
    await liveInvoke("set_active_theme", { id: made.value.id });
    await liveInvoke("get_active_theme");
    await liveInvoke("save_theme", { id: made.value.id, theme: made.value.theme });
    await liveInvoke("check_theme_draft", { id: made.value.id, theme: made.value.theme });
    expect(fetch).not.toHaveBeenCalled();
  });
});
