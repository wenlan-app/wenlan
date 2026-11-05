// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getDocument, GlobalWorkerOptions } from "pdfjs-dist/legacy/build/pdf.mjs";
import { EventBus, PDFFindController, PDFLinkService, PDFViewer } from "pdfjs-dist/legacy/web/pdf_viewer.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import "pdfjs-dist/web/pdf_viewer.css";
import "./PdfReader.css";

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
export default function PdfReader({ data, documentId, page }: {
  data: Uint8Array; documentId: string; page?: number;
}) {
  const { t } = useTranslation();
  const container = useRef<HTMLDivElement>(null);
  const pages = useRef<HTMLDivElement>(null);
  const engine = useRef<{ viewer: PDFViewer; events: EventBus } | null>(null);
  const [current, setCurrent] = useState(1);
  const [total, setTotal] = useState(0);
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<{ current: number; total: number } | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const requestedPage = useRef(page);
  requestedPage.current = page;

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
    events.on("pagesinit", () => {
      if (disposed) return;
      const saved = savedPosition(documentId);
      viewer.currentScaleValue = saved?.scale || "page-width";
      viewer.currentPageNumber = Math.max(1, Math.min(requestedPage.current ?? saved?.page ?? 1, viewer.pagesCount));
      setCurrent(viewer.currentPageNumber); setTotal(viewer.pagesCount);
    });
    events.on("pagechanging", ({ pageNumber }: { pageNumber: number }) => {
      if (!disposed) { setCurrent(pageNumber); savePosition(documentId, viewer); }
    });
    events.on("scalechanging", () => { if (!disposed) savePosition(documentId, viewer); });
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
      // The shared inspector keeps this reader alive behind its other tabs.
      // Wait for a visible width before recomputing a fit-to-width scale.
      if (!disposed && container.current?.clientWidth && viewer.pagesCount && viewer.currentScaleValue === "page-width") viewer.currentScaleValue = "page-width";
    });
    resize.observe(container.current);
    return () => {
      savePosition(documentId, viewer);
      disposed = true; resize.disconnect(); engine.current = null;
      viewer.setDocument(null); links.setDocument(null);
      void task.destroy().catch(() => {});
    };
  }, [data, documentId, attempt]);

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
  return <section className="source-pdf-reader" aria-label={t("pdfReader.title")}>
    <div className="source-pdf-tools">
      <button type="button" className="page-editor-action" aria-label={t("pdfReader.previous")} disabled={!total || current <= 1} onClick={() => go(current - 1)}>‹</button>
      <input aria-label={t("pdfReader.page")} type="number" min={1} max={total || 1} value={current} disabled={!total} onChange={e => go(Number(e.target.value))} />
      <span aria-live="polite">/ {total || "…"}</span>
      <button type="button" className="page-editor-action" aria-label={t("pdfReader.next")} disabled={!total || current >= total} onClick={() => go(current + 1)}>›</button>
      <button type="button" className="page-editor-action" aria-label={t("pdfReader.zoomOut")} disabled={!total} onClick={() => { if (engine.current) engine.current.viewer.currentScale = Math.max(.25, engine.current.viewer.currentScale / 1.2); }}>−</button>
      <button type="button" className="page-editor-action" aria-label={t("pdfReader.zoomIn")} disabled={!total} onClick={() => { if (engine.current) engine.current.viewer.currentScale = Math.min(4, engine.current.viewer.currentScale * 1.2); }}>+</button>
      <button type="button" className="page-editor-action" disabled={!total} onClick={() => { if (engine.current) engine.current.viewer.currentScaleValue = "page-width"; }}>{t("pdfReader.fit")}</button>
    </div>
    <form className="source-pdf-tools" onSubmit={e => { e.preventDefault(); search(false); }}>
      <input aria-label={t("pdfReader.search")} placeholder={t("pdfReader.search")} value={query} onChange={e => setQuery(e.target.value)} />
      <button type="submit" className="page-editor-action" disabled={!total || !query.trim()}>{t("pdfReader.search")}</button>
      <button type="button" className="page-editor-action" disabled={!total || !query.trim()} onClick={() => search(true)}>{t("pdfReader.nextMatch")}</button>
    </form>
    {failed ? <div role="alert"><p>{t("pdfReader.failed")}</p><button type="button" className="page-editor-action" onClick={() => setAttempt(n => n + 1)}>{t("sourceAccess.retry")}</button></div>
      : !total ? <p role="status">{t("pdfReader.loading")}</p>
      : matches && <p role="status">{t("pdfReader.matches", matches)}</p>}
    <div className="source-pdf-stage"><div className="source-pdf-scroll" ref={container} tabIndex={0} aria-label={t("pdfReader.title")}><div className="pdfViewer" ref={pages} /></div></div>
  </section>;
}
