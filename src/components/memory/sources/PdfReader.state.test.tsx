import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";

const harness = vi.hoisted(() => ({ viewers: [] as any[], dispatches: [] as string[], resizeCallbacks: [] as Array<() => void>, pending: true }));

vi.mock("pdfjs-dist/legacy/build/pdf.mjs", () => ({
  GlobalWorkerOptions: { workerSrc: "" },
  getDocument: () => ({ promise: harness.pending ? new Promise(() => {}) : Promise.resolve({}), destroy: async () => {} }),
}));
vi.mock("pdfjs-dist/legacy/web/pdf_viewer.mjs", () => {
  class EventBus {
    handlers = new Map<string, (event: any) => void>();
    on(name: string, handler: (event: any) => void) { this.handlers.set(name, handler); }
    dispatch(name: string, event: any) { harness.dispatches.push(name); this.handlers.get(name)?.(event); }
  }
  class PDFViewer {
    pagesCount = 0;
    private pageNumber = 1;
    get currentPageNumber() { return this.pageNumber; }
    set currentPageNumber(value: number) {
      if (this.pageNumber === value) return;
      this.pageNumber = value;
      this.eventBus.dispatch("pagechanging", { pageNumber: value });
    }
    currentScaleValue = "page-width";
    currentScale = 1;
    updateCount = 0;
    eventBus: InstanceType<typeof EventBus>;
    constructor({ eventBus }: { eventBus: InstanceType<typeof EventBus> }) {
      this.eventBus = eventBus;
      harness.viewers.push(this);
    }
    setDocument(doc: unknown) {
      if (!doc) return;
      this.pagesCount = 8;
      this.eventBus.dispatch("pagesinit", {});
    }
    getPageView() { return undefined; }
    update() { this.updateCount++; }
  }
  return {
    EventBus,
    PDFFindController: class {},
    PDFLinkService: class { externalLinkEnabled = true; setViewer() {} setDocument() {} },
    PDFViewer,
  };
});
vi.mock("pdfjs-dist/legacy/build/pdf.worker.min.mjs?url", () => ({ default: "worker.js" }));

import PdfReader from "./PdfReader";
import { readPdfReadingState, savePdfReadingState } from "./pdfReadingState";

describe("PDF reading session restoration", () => {
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { harness.resizeCallbacks.push(callback); }
    observe() {}
    disconnect() {}
  });
  afterAll(() => vi.unstubAllGlobals());

  beforeEach(() => {
    harness.viewers = [];
    harness.dispatches = [];
    harness.resizeCallbacks = [];
    harness.pending = true;
  });

  it("restores the new session page and query without rerunning search", async () => {
    savePdfReadingState("session-a", "paper-a", { page: 3, scale: "1.25", query: "alpha" });
    savePdfReadingState("session-b", "paper-b", { page: 6, scale: "1.5", query: "beta" });
    harness.pending = false;
    const view = render(<PdfReader data={new Uint8Array([1])} documentId="paper-a" readingSessionKey="session-a" />);
    expect(screen.getByRole("textbox", { name: "Find in PDF" })).toHaveValue("alpha");
    await waitFor(() => expect(harness.viewers[harness.viewers.length - 1].currentPageNumber).toBe(3));
    view.rerender(<PdfReader data={new Uint8Array([2])} documentId="paper-b" readingSessionKey="session-b" />);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Find in PDF" })).toHaveValue("beta"));
    await waitFor(() => expect(harness.viewers[harness.viewers.length - 1].currentPageNumber).toBe(6));
    expect(harness.dispatches).not.toContain("find");
    screen.getByRole("button", { name: "Find in PDF" }).click();
    expect(harness.dispatches).toContain("find");
  });

  it("does not overwrite the cached position when unmounted before PDF pages initialize", () => {
    const session = "quick-switch";
    const documentId = "paper-quick-switch";
    savePdfReadingState(session, documentId, { page: 5, scale: "1.4", query: "kept query" });
    const view = render(<PdfReader data={new Uint8Array([1])} documentId={documentId} readingSessionKey={session} />);
    expect(harness.viewers[0].pagesCount).toBe(0);
    view.unmount();
    expect(readPdfReadingState(session, documentId)).toEqual({ page: 5, scale: "1.4", query: "kept query" });
  });

  it("does not inherit another Wiki session's legacy document-global position", async () => {
    const documentId = "same-pdf-open-in-two-wikis";
    const session = "wiki-b-page";
    localStorage.setItem("wenlan.pdf-reading-positions.v1", JSON.stringify([
      [documentId, { page: 5, scale: "2" }],
    ]));
    harness.pending = false;

    render(<PdfReader data={new Uint8Array([1])} documentId={documentId} readingSessionKey={session} />);
    await waitFor(() => expect(harness.viewers[0].pagesCount).toBe(8));
    expect(harness.viewers[0].currentPageNumber).toBe(1);
    expect(harness.viewers[0].currentScaleValue).toBe("page-width");
    expect(screen.getByRole("textbox", { name: "Find in PDF" })).toHaveValue("");
  });

  it("asks PDF.js to resume rendering after a hidden reader becomes visible", async () => {
    harness.pending = false;
    const session = "reattached-wiki-page";
    const documentId = "reattached-paper";
    const view = render(<PdfReader data={new Uint8Array([1])} documentId={documentId} readingSessionKey={session} />);
    await waitFor(() => expect(harness.viewers[0].pagesCount).toBe(8));
    const scroll = view.container.querySelector(".source-pdf-scroll")!;
    const viewer = harness.viewers[0];

    Object.defineProperty(scroll, "clientWidth", { configurable: true, value: 520 });
    Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 640 });
    viewer.currentPageNumber = 3;
    expect(readPdfReadingState(session, documentId)?.page).toBe(3);
    Object.defineProperty(scroll, "clientWidth", { configurable: true, value: 0 });
    Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 0 });
    // PDF.js may report page 1 while dimensions are zero. It must not replace
    // the last position saved while the reader was visible.
    viewer.currentPageNumber = 1;
    harness.resizeCallbacks[0]();
    expect(viewer.updateCount).toBe(0);
    expect(readPdfReadingState(session, documentId)?.page).toBe(3);

    // Browsers may reset scroll position when a hidden container is expanded.
    Object.defineProperty(scroll, "clientWidth", { configurable: true, value: 520 });
    Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 640 });
    harness.resizeCallbacks[0]();
    expect(viewer.currentPageNumber).toBe(3);
    expect(viewer.updateCount).toBe(1);
  });

  it("honors an explicit citation page once before later reading position wins", async () => {
    harness.pending = false;
    const session = "hidden-citation-page";
    const documentId = "hidden-citation-paper";
    savePdfReadingState(session, documentId, { page: 3, scale: "1.2", query: "passages" });
    const view = render(<PdfReader data={new Uint8Array([1])} documentId={documentId} page={7} readingSessionKey={session} />);
    await waitFor(() => expect(harness.viewers[0].pagesCount).toBe(8));
    const scroll = view.container.querySelector(".source-pdf-scroll")!;
    const viewer = harness.viewers[0];
    expect(viewer.currentPageNumber).toBe(7);

    // The explicit jump loads while hidden, so its pagechange cannot persist.
    harness.resizeCallbacks[0]();
    expect(readPdfReadingState(session, documentId)?.page).toBe(3);
    Object.defineProperty(scroll, "clientWidth", { configurable: true, value: 520 });
    Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 640 });
    harness.resizeCallbacks[0]();
    expect(viewer.currentPageNumber).toBe(7);
    expect(readPdfReadingState(session, documentId)?.page).toBe(7);

    // Once the requested page has been honored, manual reading is preserved.
    viewer.currentPageNumber = 4;
    Object.defineProperty(scroll, "clientWidth", { configurable: true, value: 560 });
    harness.resizeCallbacks[0]();
    expect(viewer.currentPageNumber).toBe(4);
  });

  it("does not clamp an out-of-range citation to a different page", async () => {
    harness.pending = false;
    savePdfReadingState("invalid-citation", "invalid-citation-paper", { page: 3, scale: "1", query: "" });
    render(<PdfReader data={new Uint8Array([1])} documentId="invalid-citation-paper" readingSessionKey="invalid-citation" page={99} />);
    await waitFor(() => expect(harness.viewers[0].pagesCount).toBe(8));
    expect(harness.viewers[0].currentPageNumber).toBe(3);
    expect(screen.getByText("The cited PDF location is unavailable or has changed.")).toBeInTheDocument();
  });

  it("uses cached page when there is no explicit citation target", async () => {
    harness.pending = false;
    const session = "hidden-no-citation-page";
    const documentId = "hidden-no-citation-paper";
    savePdfReadingState(session, documentId, { page: 3, scale: "1.2", query: "passages" });
    const view = render(<PdfReader data={new Uint8Array([1])} documentId={documentId} readingSessionKey={session} />);
    await waitFor(() => expect(harness.viewers[0].pagesCount).toBe(8));
    const scroll = view.container.querySelector(".source-pdf-scroll")!;
    Object.defineProperty(scroll, "clientWidth", { configurable: true, value: 520 });
    Object.defineProperty(scroll, "clientHeight", { configurable: true, value: 640 });
    harness.resizeCallbacks[0]();
    expect(harness.viewers[0].currentPageNumber).toBe(3);
  });

  it("keeps navigation and zoom together, with search in its own compact row", () => {
    render(<PdfReader data={new Uint8Array([1])} documentId="compact-controls" readingSessionKey="compact-controls-wiki" />);
    const navigation = screen.getByRole("group", { name: "PDF original" });
    for (const label of ["Previous page", "Next page", "Zoom out", "Zoom in", "Fit width"]) {
      expect(within(navigation).getByRole("button", { name: label })).toBeInTheDocument();
    }
    const searchRow = document.querySelector<HTMLFormElement>(".pdf-reader-search")!;
    expect(within(searchRow).getByRole("button", { name: "Find in PDF" })).toBeInTheDocument();
    expect(within(searchRow).getByRole("button", { name: "Next match" })).toBeInTheDocument();
  });
});
