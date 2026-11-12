// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { plainTaskText, setTaskDone } from "./lookCheck";

const PAGE = [
  "# Trip",
  "",
  "- [ ] Pack the **rain** jacket",
  "- [x] Book [[Train tickets|the train]]",
  "  1. [ ] Pack the rain jacket",
  "* [ ] Two lines",
  "  carry on",
  "",
  "```",
  "- [ ] In code",
  "```",
  "- [ ] In code",
].join("\n");

describe("UX-LOOK-7 ticking a checklist item from a look", () => {
  it("ticks the named item and leaves every other line as it was", () => {
    const next = setTaskDone(PAGE, "Pack the rain jacket", 0, true)!;
    expect(next.split("\n")[2]).toBe("- [x] Pack the **rain** jacket");
    expect(next.split("\n").filter((line, i) => line !== PAGE.split("\n")[i])).toHaveLength(1);
  });

  it("picks the same-text item by its place among them", () => {
    const next = setTaskDone(PAGE, "Pack the rain jacket", 1, true)!;
    expect(next.split("\n")[4]).toBe("  1. [x] Pack the rain jacket");
    expect(next.split("\n")[2]).toBe("- [ ] Pack the **rain** jacket");
  });

  it("unticks, and matches an item by its link's words", () => {
    expect(setTaskDone(PAGE, "Book the train", 0, false)!.split("\n")[3]).toBe("- [ ] Book [[Train tickets|the train]]");
  });

  it("matches an item whose text carries on to the next line", () => {
    expect(setTaskDone(PAGE, "Two lines carry on", 0, true)!.split("\n")[5]).toBe("* [x] Two lines");
  });

  it("skips code and gives null for an item it cannot find or that is already so", () => {
    expect(setTaskDone(PAGE, "In code", 0, true)!.split("\n")[11]).toBe("- [x] In code");
    expect(setTaskDone(PAGE, "In code", 1, true)).toBeNull();
    expect(setTaskDone(PAGE, "Not on the page", 0, true)).toBeNull();
    expect(setTaskDone(PAGE, "Book the train", 0, true)).toBeNull();
  });

  it("reduces links and marks to their words", () => {
    expect(plainTaskText("See [[A#b|this]] and [docs](https://x) `now`")).toBe("See this and docs now");
  });
});
