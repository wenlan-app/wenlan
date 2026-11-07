// SPDX-License-Identifier: AGPL-3.0-only
/** Keep the visible document block in place when the reading column rewraps. */
export function preserveReadingPosition(root: HTMLElement): () => void {
  const scroller = root.closest<HTMLElement>(".wiki-workspace-content");
  const document = root.querySelector<HTMLElement>(".page-detail-document");
  if (!scroller || !document || typeof ResizeObserver === "undefined") return () => {};

  let width = document.getBoundingClientRect().width;
  let anchor: { element: HTMLElement; offset: number } | null = null;
  let scrollFrame: number | null = null;
  let settleFrame: number | null = null;
  let resizing = false;
  const capture = () => {
    const top = scroller.getBoundingClientRect().top;
    const bottom = scroller.getBoundingClientRect().bottom;
    const blocks = document.querySelectorAll<HTMLElement>(".cm-line, p, h1, h2, h3, h4, h5, h6, li, pre");
    const element = Array.from(blocks).find((block) => {
      const rect = block.getBoundingClientRect();
      return !!block.textContent?.trim() && rect.height > 0 && rect.bottom > top && rect.top < bottom;
    });
    anchor = element ? { element, offset: element.getBoundingClientRect().top - top } : null;
  };
  const restore = () => {
    if (anchor?.element.isConnected && document.contains(anchor.element)) {
      const nextOffset = anchor.element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      scroller.scrollTop += nextOffset - anchor.offset;
    }
  };
  capture();
  // A width change can clamp scrollTop before ResizeObserver runs. Keep the
  // old anchor until that observer has applied the reflow correction.
  const onScroll = () => {
    if (resizing || document.getBoundingClientRect().width !== width) return;
    // CodeMirror may render the newly visible lines after this scroll event.
    // Refresh once its viewport has caught up, before an inspector resize.
    capture();
    if (scrollFrame !== null) cancelAnimationFrame(scrollFrame);
    scrollFrame = requestAnimationFrame(() => {
      scrollFrame = null;
      if (!resizing && document.getBoundingClientRect().width === width) capture();
    });
  };
  scroller.addEventListener("scroll", onScroll, { passive: true });
  const observer = new ResizeObserver(() => {
    const next = document.getBoundingClientRect().width;
    if (next === width) return;
    resizing = true;
    if (scrollFrame !== null) {
      cancelAnimationFrame(scrollFrame);
      scrollFrame = null;
    }
    if (settleFrame !== null) cancelAnimationFrame(settleFrame);
    width = next;
    restore();
    // Keep this same block through all frames of the width transition. A
    // restore scroll event or an empty CodeMirror placeholder is not a new
    // reading position. Capture again once the width has stopped changing.
    settleFrame = requestAnimationFrame(() => {
      settleFrame = requestAnimationFrame(() => {
        settleFrame = null;
        if (document.getBoundingClientRect().width !== width) return;
        restore();
        resizing = false;
        capture();
      });
    });
  });
  observer.observe(document);
  return () => {
    observer.disconnect();
    scroller.removeEventListener("scroll", onScroll);
    if (scrollFrame !== null) cancelAnimationFrame(scrollFrame);
    if (settleFrame !== null) cancelAnimationFrame(settleFrame);
  };
}
