import { describe, expect, it } from "vitest";
import { drawPdfOverlays, uniqueQuoteRange } from "./pdfOverlays";

describe("PDF overlay geometry", () => {
  it("projects each rectangle through the viewport and replaces the old layer", () => {
    const div = document.createElement("div");
    const view = { div, viewport: { convertToViewportPoint: (x: number, y: number) => [200 - y * 2, x * 2] } };
    const annotation = { key: "A", attachment_key: "B", page_index: 1, rects: [[10, 20, 30, 40] as [number, number, number, number]], color: "#ff0000", text: "", comment: "" };
    drawPdfOverlays(view, 1, [annotation]);
    const mark = div.querySelector<HTMLElement>(".wenlan-pdf-annotation")!;
    expect(mark.style.left).toBe("120px"); expect(mark.style.top).toBe("20px");
    expect(mark.style.width).toBe("40px"); expect(mark.style.height).toBe("40px");
    drawPdfOverlays(view, 1, [annotation]);
    expect(div.querySelectorAll(".wenlan-pdf-annotation")).toHaveLength(1);
    drawPdfOverlays(view, 0, [annotation]);
    expect(div.querySelectorAll(".wenlan-pdf-annotation")).toHaveLength(0);
  });
  it("only returns a range for a unique whitespace-normalized actual text match", () => {
    const root = document.createElement("div");
    root.innerHTML = "<span>one  two </span><span>three</span>";
    expect(uniqueQuoteRange(root, "one two three")?.toString()).toBe("one  two three");
    expect(uniqueQuoteRange(root, "missing")).toBeNull();
    root.innerHTML = "<span>first</span><br><span>second</span><span>line</span>";
    expect(uniqueQuoteRange(root, "first secondline")?.toString()).toBe("firstsecondline");
    expect(uniqueQuoteRange(root, "first second line")).toBeNull();
    root.innerHTML = "<span>repeat repeat</span>";
    expect(uniqueQuoteRange(root, "repeat")).toBeNull();
  });
});
