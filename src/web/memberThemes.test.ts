// SPDX-License-Identifier: AGPL-3.0-only
import fs from "node:fs";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { liveInvoke } from "./live-invoke";
import {
  createMemberTheme, getMemberActiveTheme, getMemberTheme, listMemberThemes, resetMemberThemesForTests,
  saveMemberTheme, setMemberActiveTheme, SLOTS,
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

// The browser keeps its own copy of the built-in colors because a member's
// themes never reach a server. This keeps the copy equal to the daemon's.
describe("member theme slots", () => {
  it("match wenlan-core's built-in slot table", () => {
    const rust = fs.readFileSync(path.resolve(__dirname, "../../crates/wenlan-core/src/theme/mod.rs"), "utf8");
    const table = rust.slice(rust.indexOf("const SLOTS: &[Slot] = &["));
    const body = table.slice(0, table.indexOf("\n];"));
    const slots = [...body.matchAll(
      /Slot\s*\{\s*name:\s*"([^"]+)",\s*tokens:\s*&\[([^\]]*)\],\s*dark:\s*"([^"]+)",\s*light:\s*"([^"]+)",?\s*\}/g,
    )].map(([, name, tokens, dark, light]) => [name, [...tokens.matchAll(/"([^"]+)"/g)].map((m) => m[1]), dark, light]);
    expect(slots.length).toBeGreaterThan(10);
    expect(SLOTS).toEqual(slots);
  });
});
