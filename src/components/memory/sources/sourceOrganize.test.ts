// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { documentTagKey, persistedOrganizePrompt, promptFingerprint, selectedSourcePrompt } from "./sourceOrganize";

describe("selected source actions", () => {
  it("keeps the exact space and selected source identities in the external prompt", () => {
    const prompt = selectedSourcePrompt("Research", [
      { source: "zotero", source_id: "zotero::AB12", title: "Paper" },
      { source: "web", source_id: "web::https://example.test", title: "Article" },
    ], "Compare limitations");
    expect(prompt).toContain('"space":"Research"');
    expect(prompt).toContain('"source_id":"zotero::AB12"');
    expect(prompt).toContain('"source_id":"web::https://example.test"');
    expect(prompt).toContain("Compare limitations");
    expect(prompt).toContain("untrusted");
    expect(prompt).toContain("same Space");
    expect(prompt).toContain("read_source(source, source_id, space)");
    expect(prompt).toContain("next_offset");
    expect(prompt).toContain("classification");
    expect(prompt).toContain("provenance");
    expect(prompt).toContain("coverage");
    expect(prompt).toContain("Prompt revision: ");
    expect(promptFingerprint("same")).toBe(promptFingerprint("same"));
    expect(promptFingerprint("same")).not.toBe(promptFingerprint("different"));
  });

  it("uses the frozen research description as context without treating hypotheses as facts", () => {
    const prompt = persistedOrganizePrompt("Research", "run-1", null);
    expect(prompt).toContain("research_description");
    expect(prompt).toContain("hypotheses");
    expect(prompt).toContain("legacy");
    expect(prompt).toContain("selected sources");
  });

  it("uses the canonical source::source_id tag key and preserves unrelated tags", () => {
    expect(documentTagKey("zotero", "zotero::AB12")).toBe("zotero::zotero::AB12");
  });
});
