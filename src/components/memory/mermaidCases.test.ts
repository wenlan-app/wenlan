// SPDX-License-Identifier: AGPL-3.0-only
// The daemon checks a page's diagrams before Mermaid ever sees them
// (crates/wenlan-core/src/diagrams.rs). Its cases say whether Mermaid draws
// each one; this holds them to the real parser, so a Mermaid upgrade that
// changes an answer fails here instead of drifting.
import { describe, expect, it } from "vitest";
import mermaid from "mermaid";
import cases from "../../../crates/wenlan-core/tests/fixtures/mermaid/cases.json";

describe("diagram check cases", () => {
  it("say what the real Mermaid parser says", async () => {
    mermaid.initialize({ startOnLoad: false });
    const wrong: string[] = [];
    for (const { source, mermaid: expected } of cases) {
      const actual = await mermaid.parse(source).then(
        () => "ok",
        () => "error",
      );
      if (actual !== expected) wrong.push(`${JSON.stringify(source)}: Mermaid says ${actual}`);
    }
    expect(wrong).toEqual([]);
  });
});
