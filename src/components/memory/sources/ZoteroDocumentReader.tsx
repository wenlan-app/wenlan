// SPDX-License-Identifier: AGPL-3.0-only
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowSquareOut } from "@phosphor-icons/react";
import { getZoteroDocument, readZoteroPdf, openSearchResult } from "../../../lib/tauri";
import { toast } from "sonner";
import type { ZoteroAnnotationTarget } from "./zoteroAnnotationLink";
import { readZoteroAttachmentSelection, saveZoteroAttachmentSelection } from "./pdfReadingState";
const PdfReader = lazy(() => import("./PdfReader"));

export default function ZoteroDocumentReader({ sourceId, space, annotationTarget, readingSessionKey }: {
  sourceId: string; space?: string; annotationTarget?: ZoteroAnnotationTarget; readingSessionKey?: string;
}) {
  const { t } = useTranslation();
  const [selection, setSelection] = useState<{ attachment: string | null; page?: number; revision: number }>(() => ({
    attachment: readZoteroAttachmentSelection(readingSessionKey, sourceId, space) ?? null,
    revision: 0,
  }));
  const selectionIdentity = JSON.stringify([readingSessionKey ?? null, sourceId, space ?? null]);
  const selectionIdentityRef = useRef(selectionIdentity);
  const readerRef = useRef<HTMLElement>(null);
  const handledTarget = useRef<ZoteroAnnotationTarget | undefined>(undefined);
  const info = useQuery({ queryKey: ["zotero-document", sourceId, space ?? null],
    queryFn: () => getZoteroDocument(sourceId, space), retry: false });
  const selected = info.data?.attachments.find(a => a.key === selection.attachment)
    ?? info.data?.attachments.find(a => a.available);
  useEffect(() => {
    if (selectionIdentityRef.current !== selectionIdentity) {
      selectionIdentityRef.current = selectionIdentity;
      handledTarget.current = undefined;
      setSelection({ attachment: readZoteroAttachmentSelection(readingSessionKey, sourceId, space) ?? null, revision: 0 });
      return;
    }
    if (!info.data || !selected) return;
    // A saved attachment may have been removed from Zotero since this pane was
    // last open. Keep the selection inside the current document's inventory.
    if (selection.attachment !== selected.key) {
      setSelection(previous => ({ ...previous, attachment: selected.key }));
    }
    saveZoteroAttachmentSelection(readingSessionKey, sourceId, space, selected.key);
  }, [info.data, readingSessionKey, selected, selection.attachment, selectionIdentity, sourceId, space]);
  const pdf = useQuery({ queryKey: ["zotero-pdf", sourceId, selected?.key, space ?? null],
    queryFn: () => readZoteroPdf(sourceId, selected!.key, space),
    enabled: Boolean(selected?.available), retry: false, gcTime: 0 });
  useEffect(() => {
    if (!annotationTarget || !info.data || handledTarget.current === annotationTarget) return;
    handledTarget.current = annotationTarget;
    // A Markdown link cannot choose a file outside this document's attachment
    // inventory. The annotation id supplies provenance, never fake coordinates.
    if (!Number.isSafeInteger(annotationTarget.page) || annotationTarget.page < 1 ||
        !info.data.attachments.some(a => a.key === annotationTarget.attachmentKey)) return;
    saveZoteroAttachmentSelection(readingSessionKey, sourceId, space, annotationTarget.attachmentKey);
    setSelection(previous => ({ attachment: annotationTarget.attachmentKey, page: annotationTarget.page, revision: previous.revision + 1 }));
    readerRef.current?.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [annotationTarget, info.data]);
  if (info.isPending) return <p role="status">{t("sourceAccess.previewLoading")}</p>;
  if (info.isError) return <div role="alert"><p>{t("sourceAccess.previewFailed")}</p><button type="button" className="page-editor-action" onClick={() => void info.refetch()}>{t("sourceAccess.retry")}</button></div>;
  return <section ref={readerRef} className="zotero-document-reader">
    {info.data.warnings.length > 0 && <div role="status">
      {info.data.warnings.map((warning, index) => <p key={index}>{warning}</p>)}
    </div>}
    <div className="zotero-document-tools">
      {info.data.attachments.length > 1 && <label>{t("pdfReader.attachment")}<select value={selected?.key ?? ""} onChange={e => {
        const attachment = info.data.attachments.find(item => item.key === e.target.value);
        if (!attachment) return;
        saveZoteroAttachmentSelection(readingSessionKey, sourceId, space, attachment.key);
        setSelection(previous => ({ attachment: attachment.key, revision: previous.revision + 1 }));
      }}>
        {info.data.attachments.map(a => <option key={a.key} value={a.key}>{a.title || a.key}</option>)}
      </select></label>}
      {selected && <button type="button" className="source-pdf-open-zotero" aria-label={t("pdfReader.openZotero")} title={t("pdfReader.openZotero")} onClick={() => void openSearchResult(`zotero://open-pdf/library/items/${encodeURIComponent(selected.key)}`).catch(() => toast(t("sourceAccess.openFailed")))}><ArrowSquareOut size={14} weight="regular" aria-hidden="true" /><span>{t("pdfReader.openZotero")}</span></button>}
    </div>
    {!selected?.available ? <p>{t("pdfReader.missing")}</p>
      : pdf.isError ? <div role="alert"><p>{t("pdfReader.failed")}</p><button type="button" className="page-editor-action" onClick={() => void pdf.refetch()}>{t("sourceAccess.retry")}</button></div>
      : !pdf.data ? <p role="status">{t("pdfReader.loading")}</p>
      : <Suspense fallback={<p role="status">{t("pdfReader.loading")}</p>}><PdfReader key={selection.revision} data={pdf.data} documentId={`${sourceId}:${selected.key}`} page={selection.page} readingSessionKey={readingSessionKey} /></Suspense>}
  </section>;
}
