// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { readableSentence } from "./readableSentence";

describe("readableSentence", () => {
  it("shows a changed sentence as words, not Markdown", () => {
    expect(readableSentence("Run `pnpm test` before you tag, see [[Release checklist]].[1]"))
      .toBe("Run pnpm test before you tag, see Release checklist.");
    expect(readableSentence("Ask **Dana** or read [the guide](https://example.com) and [[Ops#Deploy|deploy notes]] [2](#citation:2)"))
      .toBe("Ask Dana or read the guide and deploy notes");
  });

  it("takes off list, task and heading marks but keeps underscores inside names", () => {
    expect(readableSentence("1. Set `WENLAN_DATA_DIR` in my_script.sh")).toBe("Set WENLAN_DATA_DIR in my_script.sh");
    expect(readableSentence("- [x] _Ship_ it")).toBe("Ship it");
    expect(readableSentence("## Windows and macOS")).toBe("Windows and macOS");
    expect(readableSentence("2 * 3 = 6")).toBe("2 * 3 = 6");
  });
});
