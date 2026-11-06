// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowRight, ArrowsOut, CaretLeft, CaretRight, MagnifyingGlass, Minus, Plus } from "@phosphor-icons/react";
import { getDocument, GlobalWorkerOptions } from "pdfjs-dist/legacy/build/pdf.mjs";
import { EventBus, PDFFindController, PDFLinkService, PDFViewer } from "pdfjs-dist/legacy/web/pdf_viewer.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import "pdfjs-dist/web/pdf_viewer.css";
import "./PdfReader.css";
import { readPdfReadingState, savePdfReadingState } from "./pdfReadingState";

GlobalWorkerOptions.workerSrc = workerUrl;
const positions = new Map<string, { page: number; scale: string }>();
const POSITION_KEY = "wenlan.pdf-reading-positions.v1";
function savedPosition(documentId: string) {
  if (positions.has(documentId)) return positions.get(documentId);
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(POSITION_KEY) ?? "[]");
    if (Array.isArray(stored)) for (const entry of stored.slice(-100)) {
      if (!Array.isArray(entry) || typeof entry[0] !== "string") continue;
      const value = entry[1];
      if (value && Number.isInteger(value.page) && value.page > 0 && typeof value.scale === "string") positions.set(entry[0], value);
    }
  } catch { /* Reading stays available when browser storage is disabled. */ }
  return positions.get(documentId);
}
function savePosition(documentId: string, viewer: PDFViewer) {
  if (!viewer.pagesCount) return;
  positions.delete(documentId);
  positions.set(documentId, { page: viewer.currentPageNumber, scale: viewer.currentScaleValue });
  if (positions.size > 100) positions.delete(positions.keys().next().value!);
  try { localStorage.setItem(POSITION_KEY, JSON.stringify([...positions])); } catch { /* Session position still survives. */ }
}

/** Viewing only: no PDF scripts, forms, editing, or automatic external links. */
export default function PdfReader({ data, documentId, page, readingSessionKey }: {
  data: Uint8Array; documentId: string; page?: number; readingSessionKey?: string;
}) {
  const { t } = useTranslation();
  const container = useRef<HTMLDivElement>(null);
  const pages = useRef<HTMLDivElement>(null);
  const engine = useRef<{ viewer: PDFViewer; events: EventBus } | null>(null);
  const [current, setCurrent] = useState(1);
  const [total, setTotal] = useState(0);
  const [query, setQuery] = useState(() => readPdfReadingState(readingSessionKey, documentId)?.query ?? "");
  const [matches, setMatches] = useState<{ current: number; total: number } | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const requestedPage = useRef(page);
  const pendingExplicitPage = useRef<number | undefined>(page);
  if (requestedPage.current !== page) {
    requestedPage.current = page;
    pendingExplicitPage.current = page;
  }
  const queryRef = useRef(query);
  queryRef.current = query;
  const readingIdentity = JSON.stringify([readingSessionKey ?? null, documentId]);
  const readingIdentityRef = useRef(readingIdentity);

  useEffect(() => {
    if (readingIdentityRef.current === readingIdentity) return;
    readingIdentityRef.current = readingIdentity;
    const nextQuery = readPdfReadingState(readingSessionKey, documentId)?.query ?? "";
    queryRef.current = nextQuery;
    setQuery(nextQuery);
    setMatches(null);
  }, [documentId, readingIdentity, readingSessionKey]);

  useEffect(() => {
    if (!container.current || !pages.current) return;
    let disposed = false;
    setFailed(false); setTotal(0); setMatches(null);
    const events = new EventBus();
    const links = new PDFLinkService({ eventBus: events });
    links.externalLinkEnabled = false;
    const find = new PDFFindController({ eventBus: events, linkService: links });
    const viewer = new PDFViewer({ container: container.current, viewer: pages.current,
      eventBus: events, linkService: links, findController: find,
      textLayerMode: 1, annotationMode: 1, removePageBorders: true });
    links.setViewer(viewer);
    engine.current = { viewer, events };
    const hasVisibleViewport = () => {
      const element = container.current;
      return Boolean(element?.isConnected && element.clientWidth > 0 && element.clientHeight > 0);
    };
    const saveSession = () => {
      // PDF.js can unmount before pagesinit (for example after a quick pane
      // switch). Its default page must not replace the prior session position.
      if (!viewer.pagesCount || !hasVisibleViewport()) return;
      savePdfReadingState(readingSessionKey, documentId, {
        page: viewer.currentPageNumber, scale: viewer.currentScaleValue, query: queryRef.current,
      });
    };
    const saveVisiblePosition = () => {
      if (!viewer.pagesCount || !hasVisibleViewport()) return;
      if (!readingSessionKey) savePosition(documentId, viewer);
      saveSession();
    };
    events.on("pagesinit", () => {
      if (disposed) return;
      const sessionState = readPdfReadingState(readingSessionKey, documentId);
      // Wiki-scoped readers must not inherit the legacy document-global
      // position: the same PDF can be open in multiple Wiki page sessions.
      const saved = readingSessionKey ? undefined : savedPosition(documentId);
      viewer.currentScaleValue = sessionState?.scale || saved?.scale || "page-width";
      viewer.currentPageNumber = Math.max(1, Math.min(requestedPage.current ?? sessionState?.page ?? saved?.page ?? 1, viewer.pagesCount));
      setCurrent(viewer.currentPageNumber); setTotal(viewer.pagesCount);
      // Restore the query in the input only. Replaying `find` here can move
      // PDF.js to a match and override the page the reader just restored.
    });
    events.on("pagechanging", ({ pageNumber }: { pageNumber: number }) => {
      if (!disposed) {
        setCurrent(pageNumber);
        if (hasVisibleViewport() && pageNumber !== pendingExplicitPage.current) pendingExplicitPage.current = undefined;
        saveVisiblePosition();
      }
    });
    events.on("scalechanging", () => { if (!disposed) saveVisiblePosition(); });
    events.on("updatefindmatchescount", ({ matchesCount }: { matchesCount: {current: number; total: number} }) => {
      if (!disposed) setMatches(matchesCount);
    });
    events.on("updatefindcontrolstate", ({ matchesCount }: { matchesCount: {current: number; total: number} }) => {
      if (!disposed) setMatches(matchesCount);
    });
    const task = getDocument({ data: data.slice(),
      cMapUrl: "/pdfjs/cmaps/", cMapPacked: true,
      standardFontDataUrl: "/pdfjs/standard_fonts/", wasmUrl: "/pdfjs/wasm/" });
    void task.promise.then((doc) => {
      if (disposed) return;
      viewer.setDocument(doc); links.setDocument(doc);
    }).catch(() => { if (!disposed) setFailed(true); });
    const resize = new ResizeObserver(() => {
      const scrollContainer = container.current;
      if (disposed || !scrollContainer || !viewer.pagesCount) return;
      if (!scrollContainer.isConnected || !scrollContainer.clientWidth || !scrollContainer.clientHeight) return;
      const sessionState = readPdfReadingState(readingSessionKey, documentId);
      const saved = readingSessionKey ? undefined : savedPosition(documentId);
      const pageToRestore = pendingExplicitPage.current ?? sessionState?.page ?? saved?.page ?? viewer.currentPageNumber;
      if (viewer.currentScaleValue === "page-width") viewer.currentScaleValue = "page-width";
      viewer.currentPageNumber = Math.max(1, Math.min(pageToRestore, viewer.pagesCount));
      // A portal can detach and reattach the reader without reloading its PDF.
      // Restore its page after dimensions return, then resume PDF.js rendering.
      viewer.update();
      saveVisiblePosition();
      pendingExplicitPage.current = undefined;
    });
    resize.observe(container.current);
    return () => {
      saveVisiblePosition();
      disposed = true; resize.disconnect(); engine.current = null;
      viewer.setDocument(null); links.setDocument(null);
      void task.destroy().catch(() => {});
    };
  }, [data, documentId, readingSessionKey, attempt]);

  useEffect(() => {
    if (page && total && engine.current) engine.current.viewer.currentPageNumber = Math.max(1, Math.min(page, total));
  }, [page, total]);
  const go = (n: number) => {
    if (Number.isFinite(n) && engine.current && total) engine.current.viewer.currentPageNumber = Math.max(1, Math.min(n, total));
  };
  const search = (again: boolean) => engine.current?.events.dispatch("find", {
    source: null, type: again ? "again" : "", query, phraseSearch: true,
    caseSensitive: false, entireWord: false, highlightAll: true, findPrevious: false,
  });
  const updateQuery = (value: string) => {
    queryRef.current = value;
    setQuery(value);
    const viewer = engine.current?.viewer;
    if (viewer?.pagesCount) savePdfReadingState(readingSessionKey, documentId, {
      page: viewer.currentPageNumber, scale: viewer.currentScaleValue, query: value,
    });
  };
  return <section className="source-pdf-reader" aria-label={t("pdfReader.title")}>
    <div className="pdf-reader-navigation" role="group" aria-label={t("pdfReader.title")}>
      <button type="button" className="pdf-reader-control" aria-label={t("pdfReader.previous")} title={t("pdfReader.previous")} disabled={!total || current <= 1} onClick={() => go(current - 1)}><CaretLeft size={16} weight="regular" aria-hidden="true" /></button>
      <input aria-label={t("pdfReader.page")} type="number" min={1} max={total || 1} value={current} disabled={!total} onChange={e => go(Number(e.target.value))} />
      <span className="pdf-reader-page-total" aria-live="polite">/ {total || "…"}</span>
      <button type="button" className="pdf-reader-control" aria-label={t("pdfReader.next")} title={t("pdfReader.next")} disabled={!total || current >= total} onClick={() => go(current + 1)}><CaretRight size={16} weight="regular" aria-hidden="true" /></button>
      <span className="pdf-reader-control-divider" aria-hidden="true" />
      <button type="button" className="pdf-reader-control" aria-label={t("pdfReader.zoomOut")} title={t("pdfReader.zoomOut")} disabled={!total} onClick={() => { if (engine.current) engine.current.viewer.currentScale = Math.max(.25, engine.current.viewer.currentScale / 1.2); }}><Minus size={16} weight="regular" aria-hidden="true" /></button>
      <button type="button" className="pdf-reader-control" aria-label={t("pdfReader.zoomIn")} title={t("pdfReader.zoomIn")} disabled={!total} onClick={() => { if (engine.current) engine.current.viewer.currentScale = Math.min(4, engine.current.viewer.currentScale * 1.2); }}><Plus size={16} weight="regular" aria-hidden="true" /></button>
      <button type="button" className="pdf-reader-control" aria-label={t("pdfReader.fit")} title={t("pdfReader.fit")} disabled={!total} onClick={() => { if (engine.current) engine.current.viewer.currentScaleValue = "page-width"; }}><ArrowsOut size={16} weight="regular" aria-hidden="true" /></button>
    </div>
    <form className="pdf-reader-search" onSubmit={e => { e.preventDefault(); search(false); }}>
      <input aria-label={t("pdfReader.search")} placeholder={t("pdfReader.search")} value={query} onChange={e => updateQuery(e.target.value)} />
      <button type="submit" className="pdf-reader-control" aria-label={t("pdfReader.search")} title={t("pdfReader.search")} disabled={!total || !query.trim()}><MagnifyingGlass size={16} weight="regular" aria-hidden="true" /></button>
      <button type="button" className="pdf-reader-control" aria-label={t("pdfReader.nextMatch")} title={t("pdfReader.nextMatch")} disabled={!total || !query.trim()} onClick={() => search(true)}><ArrowRight size={16} weight="regular" aria-hidden="true" /></button>
    </form>
    {failed ? <div role="alert"><p>{t("pdfReader.failed")}</p><button type="button" className="page-editor-action" onClick={() => setAttempt(n => n + 1)}>{t("sourceAccess.retry")}</button></div>
      : !total ? <p role="status">{t("pdfReader.loading")}</p>
      : matches && <p role="status">{t("pdfReader.matches", matches)}</p>}
    <div className="source-pdf-stage"><div className="source-pdf-scroll" ref={container} tabIndex={0} aria-label={t("pdfReader.title")}><div className="pdfViewer" ref={pages} /></div></div>
  </section>;
}
