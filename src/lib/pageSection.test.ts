// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { clearPageSection, headingKey, pendingPageSection, requestPageSection, wikilinkHeading } from "./pageSection";

describe("pageSection", () => {
  it("holds a link's heading for the page it opens, and only that page", () => {
    requestPageSection("p2", " Third chapter ");
    expect(pendingPageSection("p1")).toBeNull();
    expect(pendingPageSection("p2")).toBe("Third chapter");
    clearPageSection("p1");
    expect(pendingPageSection("p2")).toBe("Third chapter");
    clearPageSection("p2");
    expect(pendingPageSection("p2")).toBeNull();
    // A plain link replaces a heading still waiting from an earlier click.
    requestPageSection("p2", "Old");
    requestPageSection("p3", null);
    expect(pendingPageSection("p2")).toBeNull();
  });

  it("reads a link's heading and compares headings by their words", () => {
    expect(wikilinkHeading("Book#Third chapter|there")).toBe("Third chapter");
    expect(wikilinkHeading("Book|a#b")).toBeNull();
    expect(wikilinkHeading("Book")).toBeNull();
    expect(headingKey("## Third  Chapter ##")).toBe(headingKey("third chapter"));
  });
});
