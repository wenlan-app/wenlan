// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { splitPageLayers } from "./pageLayers";

const PAGE = [
  "Wenlan is a place to think with AI.",
  "",
  "## Three views",
  "",
  "Short text, mind map, any shape you want.",
  "",
  "## 給 AI 的細節",
  "",
  "- 10/30 21:04 source = which version of which page.",
].join("\n");

describe("splitPageLayers", () => {
  it("UX-VIEW-3 keeps the part for AI out of the human part", () => {
    const layers = splitPageLayers(PAGE);
    expect(layers.human).toBe("Wenlan is a place to think with AI.\n\n## Three views\n\nShort text, mind map, any shape you want.");
    expect(layers.aiDetails).toBe("- 10/30 21:04 source = which version of which page.");
  });

  it("UX-VIEW-2 cuts the short length before the first section", () => {
    expect(splitPageLayers(PAGE).short).toBe("Wenlan is a place to think with AI.");
  });

  it("UX-VIEW-2 gives one length when the page has no sections", () => {
    const layers = splitPageLayers("Just one paragraph.");
    expect(layers.short).toBe(layers.human);
    expect(layers.aiDetails).toBe("");
  });

  it("recognises the marker in every interface language and ignores headings in code", () => {
    expect(splitPageLayers("Top\n\n## Details for AI\n\nx").aiDetails).toBe("x");
    expect(splitPageLayers("Top\n\n## 给 AI 的细节\n\nx").aiDetails).toBe("x");
    const fenced = splitPageLayers("Intro\n\n```md\n## not a section\n```\n\nMore");
    expect(fenced.short).toBe(fenced.human);
  });
});
