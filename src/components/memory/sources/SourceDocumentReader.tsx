// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { getChunks, openSearchResult, type IndexedFileInfo } from "../../../lib/tauri";
import { DetailInfoContent, DetailReadingFrame } from "../DetailLayout";
import PageInfoDrawer from "../page/PageInfoDrawer";
import { InfoToggle } from "../page/InfoToggle";
import { useDetailInfoOpen } from "../useDetailInfoOpen";
import SourceDocumentContent from "./SourceDocumentContent";
import SourceAnnotations from "./SourceAnnotations";
import { getSourcePresentation, getSourceAppFromUrl, sourceAppName } from "./sourcePresentation";
import { SourceTypeIcon } from "./SourceTypeIcon";
import "../readingSurface.css";
import "../page/detailInfo.css";
import "../context/knowledge-context.css";
import "../page/pageDocumentTools.css";
import "./SourceDocumentReader.css";

function safeOriginalUrl(file: IndexedFileInfo): string | null {
  try {
    const parsed = new URL(file.source === "webpage" ? file.source_id : (file.url ?? ""));
    if (["http:", "https:"].includes(parsed.protocol) && !parsed.username && !parsed.password) return parsed.href;
  } catch {
    // A non-web or malformed source has no open-original action.
  }
  return null;
}

export function SourceDocumentReader({ file, space, onReady, onRegisterFlush, frame = false, renderRelatedPages }: {
  file: IndexedFileInfo;
  space?: string;
  onReady?: () => void;
  onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
  frame?: boolean;
  renderRelatedPages?: (content: ReactNode) => ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const source = ["file", "webpage"].includes(file.source) ? file.source : "memory";
  const isWebpage = file.source === "webpage";
  const chunks = useQuery({
    queryKey: ["sourceDocument", space ?? null, source, file.source_id],
    queryFn: () => space === undefined ? getChunks(source, file.source_id) : getChunks(source, file.source_id, space),
    enabled: !isWebpage,
    retry: false,
  });
  useEffect(() => { if (!isWebpage && !chunks.isPending) onReady?.(); }, [isWebpage, chunks.isPending, onReady]);
  const originalUrl = safeOriginalUrl(file);
  const app = isWebpage ? getSourceAppFromUrl(originalUrl) : null;
  const content = [...(chunks.data ?? [])].sort((a, b) => a.chunk_index - b.chunk_index).map((chunk) => chunk.content).join("\n\n");

  return <div className={`source-document-reader${frame ? " reading-document-frame" : ""}`}>
    {originalUrl && <div className="source-document-reader-url">
      {app && <span className="source-document-reader-app" data-source-app={app}>
        <SourceTypeIcon kind={app} size={16} />{sourceAppName(app, i18n.language)}
      </span>}
      <a href={originalUrl} aria-label={t("sourceAccess.openOriginal")} onClick={(event) => {
        event.preventDefault();
        void openSearchResult(originalUrl).catch(() => toast(t("sourceAccess.openFailed")));
      }}>
        <span>{originalUrl}</span>
        <svg aria-hidden="true" viewBox="0 0 16 16" focusable="false"><path d="M6 3h7v7M13 3 7 9M11 9v4H3V5h4" /></svg>
      </a>
    </div>}
    {isWebpage ? <SourceAnnotations key={JSON.stringify(["webpage", file.source_id, space ?? null])}
      sourceId={file.source_id} space={space} onReady={onReady} onRegisterFlush={onRegisterFlush} renderRelatedPages={renderRelatedPages} />
      : chunks.isPending ? <p role="status" className="source-document-reader-note">{t("sourceAccess.previewLoading")}</p>
      : chunks.isError && !chunks.data ? <div role="alert" className="source-document-reader-error">
        <p className="source-document-reader-note">{t("sourceAccess.previewFailed")}</p>
        <button type="button" className="page-editor-action" onClick={() => void chunks.refetch()}>{t("sourceAccess.retry")}</button>
      </div>
      : content ? <SourceDocumentContent content={content} title={file.title} />
      : <p className="source-document-reader-note">{t("sourceAccess.noPreview")}</p>}
  </div>;
}

/** Shared source document shell for indexed sources and full local-file reads. */
export function SourceDocumentLayout({ file, space, onReady, onRegisterFlush, content, infoContent, breadcrumbs }: {
  file: IndexedFileInfo;
  space?: string;
  onReady?: () => void;
  onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
  content?: ReactNode;
  infoContent?: ReactNode;
  breadcrumbs?: ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const { open, setOpen, toggle } = useDetailInfoOpen(JSON.stringify([file.source, file.source_id, space ?? null]));
  const infoToggleRef = useRef<HTMLButtonElement>(null);
  const presentation = getSourcePresentation(file, { document: t("sourceLibrary.document"), webExcerpt: t("sourceLibrary.webExcerpt"), language: i18n.language });
  const sourceSpace = space ?? file.space;
  const modified = Number.isFinite(file.last_modified) && file.last_modified > 0
    ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(file.last_modified * 1000)) : null;
  const info = (relatedPages: ReactNode) => <PageInfoDrawer docked inspector focusOnOpen={false} returnFocusRef={infoToggleRef}
    open={open} onClose={() => setOpen(false)} title={t("knowledgeContext.memoryPanel")} closeLabel={t("common.close")}>
    <DetailInfoContent className="source-document-info">
      {relatedPages}
      <section>
        <h3 className="note-info-section-heading">{t("sourceAccess.details")}</h3>
        <dl className="source-info-metadata">
          <div><dt>{t("sourceAccess.type")}</dt><dd>{presentation.app ? presentation.format : file.source === "webpage" ? t("sourceAccess.link") : presentation.format}</dd></div>
          {presentation.origin && <div><dt>{t(file.source === "webpage" ? "sourceAccess.website" : "sourceAccess.folderPath")}</dt><dd>{presentation.origin}</dd></div>}
          {sourceSpace && <div><dt>{t("sourceAccess.space")}</dt><dd>{sourceSpace}</dd></div>}
          {modified && <div><dt>{t("sourceAccess.updated")}</dt><dd>{modified}</dd></div>}
        </dl>
      </section>
      {infoContent}
    </DetailInfoContent>
  </PageInfoDrawer>;
  return <article className={`page-detail source-document-page document-context-host${open ? " document-context-open" : ""}`}>
    <DetailReadingFrame variant={file.source === "webpage" ? "compact" : "long-form"}>
      {breadcrumbs && <div className="reading-document-breadcrumbs">{breadcrumbs}</div>}
      <header className="page-document-title-row source-document-page-header">
        <h1 className="page-detail-title">{file.title}</h1>
        <div className="page-document-tools"><InfoToggle ref={infoToggleRef} open={open} onClick={toggle} /></div>
      </header>
      {content !== undefined ? content : <SourceDocumentReader file={file} space={space} onReady={onReady} onRegisterFlush={onRegisterFlush} renderRelatedPages={info} />}
    </DetailReadingFrame>
    {(content !== undefined || file.source !== "webpage") && info(null)}
  </article>;
}

export function SourceDocumentPage({ file, space, onReady, onRegisterFlush }: { file: IndexedFileInfo; space?: string; onReady?: () => void; onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void }) {
  const scope = space ?? file.space ?? undefined;
  return <SourceDocumentLayout file={file} {...(scope === undefined ? {} : { space: scope })} {...(onReady === undefined ? {} : { onReady })} {...(onRegisterFlush === undefined ? {} : { onRegisterFlush })} />;
}
