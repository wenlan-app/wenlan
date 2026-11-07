// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, expect, it, vi } from "vitest";
import { preserveReadingPosition } from "./readingReflowAnchor";

function rect(top: number, bottom: number, width = 500): DOMRect {
  return { top, bottom, width, height: bottom - top, x: 0, y: top, left: 0, right: width, toJSON: () => ({}) } as DOMRect;
}

afterEach(() => { document.body.replaceChildren(); vi.unstubAllGlobals(); });

it("keeps the current visible block offset when the reading width changes", () => {
  let notify: ResizeObserverCallback = () => {};
  const disconnect = vi.fn();
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: ResizeObserverCallback) { notify = callback; }
    observe() {} disconnect = disconnect;
  });
  const scroller = document.createElement("div");
  scroller.className = "wiki-workspace-content";
  scroller.innerHTML = '<div class="page-detail"><div class="page-detail-document"><p>First</p><p>Current paragraph</p></div></div>';
  document.body.append(scroller);
  const root = scroller.querySelector<HTMLElement>(".page-detail")!;
  const frame = scroller.querySelector<HTMLElement>(".page-detail-document")!;
  const [first, current] = Array.from(frame.querySelectorAll("p"));
  let width = 500;
  let currentTop = 20;
  vi.spyOn(scroller, "getBoundingClientRect").mockImplementation(() => rect(0, 300));
  vi.spyOn(frame, "getBoundingClientRect").mockImplementation(() => rect(-300, 1000, width));
  vi.spyOn(first, "getBoundingClientRect").mockImplementation(() => rect(-200, -100));
  vi.spyOn(current, "getBoundingClientRect").mockImplementation(() => rect(currentTop, currentTop + 80));
  scroller.scrollTop = 400;
  const cleanup = preserveReadingPosition(root);
  width = 350;
  currentTop = 160;
  notify([], {} as ResizeObserver);
  expect(scroller.scrollTop).toBe(540);
  cleanup();
  expect(disconnect).toHaveBeenCalledOnce();
});

it("keeps the pre-reflow anchor when widening clamps scroll before resize notification", () => {
  let notify: ResizeObserverCallback = () => {};
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: ResizeObserverCallback) { notify = callback; }
    observe() {} disconnect() {}
  });
  const scroller = document.createElement("div");
  scroller.className = "wiki-workspace-content";
  scroller.innerHTML = '<div class="page-detail"><div class="page-detail-document"><p>Original paragraph</p><p>Later paragraph</p></div></div>';
  document.body.append(scroller);
  const root = scroller.querySelector<HTMLElement>(".page-detail")!;
  const frame = scroller.querySelector<HTMLElement>(".page-detail-document")!;
  const [original, later] = Array.from(frame.querySelectorAll("p"));
  let width = 472;
  let originalTop = 43;
  let laterTop = 900;
  vi.spyOn(scroller, "getBoundingClientRect").mockImplementation(() => rect(0, 500));
  vi.spyOn(frame, "getBoundingClientRect").mockImplementation(() => rect(-7000, 3000, width));
  vi.spyOn(original, "getBoundingClientRect").mockImplementation(() => rect(originalTop, originalTop + 100));
  vi.spyOn(later, "getBoundingClientRect").mockImplementation(() => rect(laterTop, laterTop + 100));
  scroller.scrollTop = 7298;
  const cleanup = preserveReadingPosition(root);
  width = 794;
  scroller.scrollTop = 5291;
  originalTop = -640;
  laterTop = 57;
  scroller.dispatchEvent(new Event("scroll"));
  notify([], {} as ResizeObserver);
  expect(scroller.scrollTop).toBe(4608);
  // The programmatic scroll event may run after the correction; it must
  // capture the corrected position for the next resize.
  originalTop = 43;
  laterTop = 740;
  scroller.dispatchEvent(new Event("scroll"));
  width = 600;
  originalTop = 100;
  notify([], {} as ResizeObserver);
  expect(scroller.scrollTop).toBe(4665);
  cleanup();
});
