// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import {
  overviewLabelMaxWidth,
  selectOverviewFallbackLabels,
  truncateOverviewLabel,
} from "./overviewLabels";

describe("overview label sizing", () => {
  const measured = {
    measureText: (text: string) => ({ width: Array.from(text).length * 10 }),
  } as unknown as Pick<CanvasRenderingContext2D, "measureText">;

  it("keeps a label intact when its measured width fits", () => {
    expect(truncateOverviewLabel(measured, "Atlas", 60)).toBe("Atlas");
  });

  it("truncates by measured canvas width and preserves the ellipsis", () => {
    expect(truncateOverviewLabel(measured, "Atlas network", 60)).toBe("Atlas…");
    expect(measured.measureText(truncateOverviewLabel(measured, "Atlas network", 60)).width).toBeLessThanOrEqual(60);
  });

  it("cuts at a word's end, never part way through one, while CJK may end anywhere", () => {
    expect(truncateOverviewLabel(measured, "Pricing page stays open", 100)).toBe("Pricing…");
    expect(truncateOverviewLabel(measured, "Tagline, pricing", 100)).toBe("Tagline…");
    // One long word has no end to go back to: cut where it must.
    expect(truncateOverviewLabel(measured, "Supercalifragilistic", 60)).toBe("Super…");
    expect(truncateOverviewLabel(measured, "週末重寫了定價頁", 60)).toBe("週末重寫了…");
  });

  it("uses a smaller readable budget on narrow viewports", () => {
    expect(overviewLabelMaxWidth(1200)).toBe(160);
    expect(overviewLabelMaxWidth(320)).toBe(134.4);
  });

  it("caps fallback names while distributing them by stable positions", () => {
    const candidates = Array.from({ length: 20 }, (_, index) => ({
      id: `node-${String(index).padStart(2, "0")}`,
      x: index % 5,
      y: Math.floor(index / 5),
    }));
    const first = selectOverviewFallbackLabels(candidates, 12);
    const second = selectOverviewFallbackLabels([...candidates].reverse(), 12);

    expect(first.size).toBe(12);
    expect(second).toEqual(first);
    expect([...first].some((id) => id === "node-19")).toBe(true);
  });
});
