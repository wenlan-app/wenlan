// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it } from "vitest";
import { PINNED_PAGES_PREFERENCE_KEY, isPagePinned, movePinnedPage, pinPage, readPinnedPages, renamePinnedPage, unpinPage } from "./pinnedPages";

describe("pinned pages", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("UX-FIND-6: starts empty, so the left side is unchanged until a page is pinned", () => {
    expect(readPinnedPages()).toEqual([]);
  });

  it("pins once, keeps order, and unpinning removes only the row", () => {
    pinPage({ id: "a", title: "Skills" });
    pinPage({ id: "b", title: "Agents" });
    pinPage({ id: "a", title: "Skills again" });
    expect(readPinnedPages()).toEqual([{ id: "a", title: "Skills" }, { id: "b", title: "Agents" }]);
    expect(isPagePinned("a")).toBe(true);
    unpinPage("a");
    expect(readPinnedPages()).toEqual([{ id: "b", title: "Agents" }]);
    expect(JSON.parse(localStorage.getItem(PINNED_PAGES_PREFERENCE_KEY) ?? "[]")).toEqual([{ id: "b", title: "Agents" }]);
  });

  it("moves a pin before another, or to the end", () => {
    for (const id of ["a", "b", "c"]) pinPage({ id, title: id });
    movePinnedPage("c", "a");
    expect(readPinnedPages().map((page) => page.id)).toEqual(["c", "a", "b"]);
    movePinnedPage("c", null);
    expect(readPinnedPages().map((page) => page.id)).toEqual(["a", "b", "c"]);
  });

  it("follows a page's new name and ignores pages that are not pinned", () => {
    pinPage({ id: "a", title: "Old" });
    renamePinnedPage("a", "New");
    renamePinnedPage("z", "Nope");
    expect(readPinnedPages()).toEqual([{ id: "a", title: "New" }]);
  });

  it("reads a damaged or foreign payload as no pins", () => {
    localStorage.setItem(PINNED_PAGES_PREFERENCE_KEY, "{not json");
    expect(readPinnedPages()).toEqual([]);
    localStorage.setItem(PINNED_PAGES_PREFERENCE_KEY, JSON.stringify([{ id: "a", title: "A" }, { id: "a", title: "dup" }, { id: 3 }, null]));
    expect(readPinnedPages()).toEqual([{ id: "a", title: "A" }]);
  });
});
