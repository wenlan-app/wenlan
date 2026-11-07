// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it } from "vitest";
import { readSourceSelection, writeSourceSelection } from "./sourceSelectionState";

describe("source selection session state", () => {
  beforeEach(() => sessionStorage.clear());

  it("persists a deduplicated, bounded selection across view remounts", () => {
    writeSourceSelection("space-1", ["a", "a", ...Array.from({ length: 150 }, (_, i) => `source-${i}`)]);
    expect(readSourceSelection("space-1")).toHaveLength(100);
    expect(readSourceSelection("space-1")[0]).toBe("a");
  });
});
