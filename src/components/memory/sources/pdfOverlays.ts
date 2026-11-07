// SPDX-License-Identifier: AGPL-3.0-only
import type { ZoteroPdfAnnotation } from "../../../lib/tauri";

type Viewport = { convertToViewportPoint(x: number, y: number): number[] };
type PageView = { div: HTMLElement; viewport: Viewport };

/** PDF.js may discard custom page children when it resets a page. */
export function drawPdfOverlays(view: PageView, pageIndex: number, annotations: ZoteroPdfAnnotation[], quote?: string) {
  view.div.querySelector(":scope > .wenlan-pdf-overlays")?.remove();
  const layer = document.createElement("div");
  layer.className = "wenlan-pdf-overlays";
  for (const annotation of annotations) {
    if (annotation.page_index !== pageIndex || !/^#[0-9a-f]{6}$/i.test(annotation.color)) continue;
    for (const rect of annotation.rects) {
      if (rect.length !== 4 || !rect.every(Number.isFinite)) continue;
      const a = view.viewport.convertToViewportPoint(rect[0], rect[1]);
      const b = view.viewport.convertToViewportPoint(rect[2], rect[3]);
      if (![...a, ...b].every(Number.isFinite)) continue;
      const width = Math.abs(a[0] - b[0]), height = Math.abs(a[1] - b[1]);
      if (!width || !height) continue;
      const mark = document.createElement("div");
      mark.className = "wenlan-pdf-annotation";
      Object.assign(mark.style, { left: `${Math.min(a[0], b[0])}px`, top: `${Math.min(a[1], b[1])}px`, width: `${width}px`, height: `${height}px`, backgroundColor: annotation.color });
      layer.append(mark);
    }
  }
  if (quote) {
    const textLayer = view.div.querySelector<HTMLElement>(".textLayer");
    const range = textLayer && uniqueQuoteRange(textLayer, quote);
    if (range && typeof range.getClientRects === "function") {
      const pageRect = view.div.getBoundingClientRect();
      for (const rect of Array.from(range.getClientRects())) {
        if (!rect.width || !rect.height) continue;
        const mark = document.createElement("div");
        mark.className = "wenlan-pdf-quote";
        Object.assign(mark.style, { left: `${rect.left - pageRect.left}px`, top: `${rect.top - pageRect.top}px`, width: `${rect.width}px`, height: `${rect.height}px` });
        layer.append(mark);
      }
    }
  }
  if (layer.childElementCount) view.div.append(layer);
}

/** Match only a unique, whitespace-normalized quote in the actual PDF.js text layer. */
export function uniqueQuoteRange(root: HTMLElement, quote: string): Range | null {
  const needle = quote.replace(/\s+/g, " ").trim();
  if (!needle) return null;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
  const chars: ({ node: Text; offset: number } | null)[] = [];
  let normalized = "", previousSpace = true;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node.nodeType === Node.ELEMENT_NODE) {
      // PDF.js emits <br> for real line breaks; adjacent glyph spans do not
      // themselves imply a space.
      if ((node as Element).tagName === "BR" && !previousSpace) { normalized += " "; chars.push(null); previousSpace = true; }
      continue;
    }
    const text = node as Text;
    for (let i = 0; i < text.data.length; i++) {
      const ch = text.data[i];
      if (/\s/.test(ch)) {
        if (previousSpace) continue;
        normalized += " "; previousSpace = true;
      } else { normalized += ch; previousSpace = false; }
      chars.push({ node: text, offset: i });
    }
  }
  const start = normalized.indexOf(needle);
  if (start < 0 || normalized.indexOf(needle, start + 1) !== -1) return null;
  const first = chars[start], last = chars[start + needle.length - 1];
  if (!first || !last) return null;
  const range = document.createRange();
  range.setStart(first.node, first.offset);
  range.setEnd(last.node, last.offset + 1);
  return range;
}
