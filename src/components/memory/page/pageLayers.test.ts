// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { pageLayerOffsets, splitPageLayers } from "./pageLayers";

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

  it("UX-VIEW-3 splits at the anchor the app writes, whatever the heading under it says", () => {
    const layers = splitPageLayers("Top\n\n<!-- wenlan:details -->\n## Notes\n\nx");
    expect(layers.human).toBe("Top");
    expect(layers.aiDetails).toBe("x");
  });

  it("UX-VIEW-2 skips leading headings and keeps the first paragraph as the short length", () => {
    expect(splitPageLayers("# Title\n\nFirst line.\nSame paragraph.\n\nSecond paragraph.").short).toBe(
      "# Title\n\nFirst line.\nSame paragraph.",
    );
  });

  it("recognises the marker in every interface language and ignores headings in code", () => {
    expect(splitPageLayers("Top\n\n## Details for AI\n\nx").aiDetails).toBe("x");
    expect(splitPageLayers("Top\n\n## 给 AI 的细节\n\nx").aiDetails).toBe("x");
    expect(splitPageLayers("Top\n\n## Detalles para la IA\n\nx").aiDetails).toBe("x");
    expect(splitPageLayers("Top\n\n##  details  FOR ai\n\nx").aiDetails).toBe("x");
    expect(splitPageLayers("Top\n\n### Details for AI\n\nx").aiDetails).toBe("");
    const fenced = splitPageLayers("Intro\n\n```md\n## Details for AI\n```\n\nMore");
    expect(fenced.aiDetails).toBe("");
    expect(fenced.human).toContain("More");
  });
});

describe("pageLayerOffsets", () => {
  it("UX-VIEW-3 finds where the short length ends and the part for AI starts", () => {
    const offsets = pageLayerOffsets(PAGE);
    expect(PAGE.slice(offsets.sectionStart ?? 0).trimStart()).toMatch(/^## Three views/);
    expect(PAGE.slice(offsets.aiStart ?? 0)).toMatch(/^## 給 AI 的細節/);
  });

  it("UX-VIEW-2 reports one length when there is nothing to cut", () => {
    expect(pageLayerOffsets("Just one paragraph.")).toEqual({ sectionStart: null, aiStart: null });
    expect(pageLayerOffsets("## Only a section\n\nText").sectionStart).toBeNull();
    expect(pageLayerOffsets("Lede\n\n## Details for AI\n\nx").sectionStart).toBeNull();
  });
});
