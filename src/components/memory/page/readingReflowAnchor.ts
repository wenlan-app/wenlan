// SPDX-License-Identifier: AGPL-3.0-only
/** Keep the visible document block in place when the reading column rewraps. */
export function preserveReadingPosition(root: HTMLElement): () => void {
  const scroller = root.closest<HTMLElement>(".wiki-workspace-content");
  const document = root.querySelector<HTMLElement>(".page-detail-document");
  if (!scroller || !document || typeof ResizeObserver === "undefined") return () => {};

  let width = document.getBoundingClientRect().width;
  let anchor: { element: HTMLElement; offset: number } | null = null;
  const capture = () => {
    const top = scroller.getBoundingClientRect().top;
    const bottom = scroller.getBoundingClientRect().bottom;
    const blocks = document.querySelectorAll<HTMLElement>(".cm-line, p, h1, h2, h3, h4, h5, h6, li, pre");
    const element = Array.from(blocks).find((block) => {
      const rect = block.getBoundingClientRect();
      return rect.height > 0 && rect.bottom > top && rect.top < bottom;
    });
    anchor = element ? { element, offset: element.getBoundingClientRect().top - top } : null;
  };
  capture();
  // A width change can clamp scrollTop before ResizeObserver runs. Keep the
  // old anchor until that observer has applied the reflow correction.
  const onScroll = () => {
    if (document.getBoundingClientRect().width === width) capture();
  };
  scroller.addEventListener("scroll", onScroll, { passive: true });
  const observer = new ResizeObserver(() => {
    const next = document.getBoundingClientRect().width;
    if (next === width) return;
    width = next;
    if (anchor?.element.isConnected && document.contains(anchor.element)) {
      const nextOffset = anchor.element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      scroller.scrollTop += nextOffset - anchor.offset;
    }
    capture();
  });
  observer.observe(document);
  return () => {
    observer.disconnect();
    scroller.removeEventListener("scroll", onScroll);
  };
}
