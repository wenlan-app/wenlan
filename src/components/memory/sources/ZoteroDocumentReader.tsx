// SPDX-License-Identifier: AGPL-3.0-only
import { safePublicSourceUrl } from "../../../lib/publicSourceUrl";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowSquareOut } from "@phosphor-icons/react";
import { getZoteroDocument, readZoteroPdf, openSearchResult, syncRegisteredSource, type PageCitation } from "../../../lib/tauri";
import { toast } from "sonner";
import type { ZoteroAnnotationTarget } from "./zoteroAnnotationLink";
import { readZoteroAttachmentSelection, saveZoteroAttachmentSelection } from "./pdfReadingState";
const PdfReader = lazy(() => import("./PdfReader"));


export default function ZoteroDocumentReader({ sourceId, space, annotationTarget, readingSessionKey, citation }: {
  sourceId: string; space?: string; annotationTarget?: ZoteroAnnotationTarget; readingSessionKey?: string; citation?: PageCitation;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [selection, setSelection] = useState<{ attachment: string | null; page?: number; revision: number }>(() => ({
    attachment: readZoteroAttachmentSelection(readingSessionKey, sourceId, space) ?? null,
    revision: 0,
  }));
  const selectionIdentity = JSON.stringify([readingSessionKey ?? null, sourceId, space ?? null]);
  const selectionIdentityRef = useRef(selectionIdentity);
  const readerRef = useRef<HTMLElement>(null);
  const handledTarget = useRef<ZoteroAnnotationTarget | undefined>(undefined);
  const handledCitation = useRef<{ citation: PageCitation; attachmentState: string } | undefined>(undefined);
  const [citationWarning, setCitationWarning] = useState(false);
  const frozen = citation?.pdf_target;
  const info = useQuery({ queryKey: ["zotero-document", sourceId, space ?? null, frozen?.attachment_key ?? null, frozen?.pdf_digest ?? null],
    queryFn: () => frozen ? getZoteroDocument(sourceId, space, frozen.attachment_key, frozen.pdf_digest, true) : getZoteroDocument(sourceId, space), retry: false });
  const sync = useMutation({
    mutationFn: async (registeredSourceId: string) => {
      const result = await syncRegisteredSource(registeredSourceId);
      await info.refetch();
      await queryClient.invalidateQueries({ queryKey: ["zotero-pdf", sourceId] });
      if (result.errors > 0) throw new Error("Zotero sync incomplete");
    },
  });
  const selected = frozen ? info.data?.attachments.find(a => a.key === frozen.attachment_key && a.pdf_digest === frozen.pdf_digest) : info.data?.attachments.find(a => a.key === selection.attachment)
    ?? info.data?.attachments.find(a => a.available)
    ?? info.data?.attachments[0];
  useEffect(() => {
    if (selectionIdentityRef.current !== selectionIdentity) {
      selectionIdentityRef.current = selectionIdentity;
      handledTarget.current = undefined;
      handledCitation.current = undefined;
      setCitationWarning(false);
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
  const pdf = useQuery({ queryKey: ["zotero-pdf", sourceId, selected?.key, space ?? null, selected?.pdf_digest ?? null, selected?.pdf_digest ? null : info.dataUpdatedAt, !!frozen],
    queryFn: () => frozen ? readZoteroPdf(sourceId, selected!.key, space, frozen.pdf_digest, true) : readZoteroPdf(sourceId, selected!.key, space, selected?.pdf_digest),
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
  useEffect(() => {
    if (!citation || !info.data) return;
    const target = citation.pdf_target;
    if (!target) {
      handledCitation.current = undefined;
      setCitationWarning(false);
      return;
    }
    const attachment = target && info.data.attachments.find(a => a.key === target.attachment_key);
    const attachmentState = JSON.stringify([attachment?.key ?? null, attachment?.available ?? false, attachment?.pdf_digest ?? null]);
    if (handledCitation.current?.citation === citation && handledCitation.current.attachmentState === attachmentState) return;
    handledCitation.current = { citation, attachmentState };
    if (!attachment || !attachment.available || !attachment.pdf_digest ||
        attachment.pdf_digest !== target.pdf_digest || !Number.isSafeInteger(target.page) || target.page < 1) {
      setCitationWarning(true);
      return;
    }
    setCitationWarning(false);
    saveZoteroAttachmentSelection(readingSessionKey, sourceId, space, attachment.key);
    setSelection(previous => ({ attachment: attachment.key, page: target.page, revision: previous.revision + 1 }));
    readerRef.current?.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [citation, info.data, readingSessionKey, sourceId, space]);
  if (info.isPending) return <p role="status">{t("sourceAccess.previewLoading")}</p>;
  if (info.isError) return <div role="alert"><p>{t(frozen ? "pdfReader.historicalUnavailable" : "sourceAccess.previewFailed")}</p><button type="button" className="page-editor-action" onClick={() => void info.refetch()}>{t("sourceAccess.retry")}</button></div>;
  return <section ref={readerRef} className="zotero-document-reader">
    {info.data.historical === true && <p role="status">{t("pdfReader.historical")}</p>}
    {info.data.warnings.length > 0 && <div role="status">
      {info.data.warnings.map((warning, index) => <p key={index}>{warning}</p>)}
    </div>}
    {citation && !citation.pdf_target && <p role="status">{t("pdfReader.locationNotSpecified")}</p>}
    {citation?.pdf_target && citationWarning && <p role="status">{t("pdfReader.locationUnavailable")}</p>}
    <div className="zotero-document-tools">
      {safePublicSourceUrl(info.data.source_url) && <button type="button" className="source-pdf-open-zotero" onClick={() => void openSearchResult(safePublicSourceUrl(info.data.source_url)!).catch(() => toast(t("sourceAccess.openFailed")))}>{t("pdfReader.openSourcePage")}</button>}
      {!frozen && info.data.attachments.length > 1 && <label>{t("pdfReader.attachment")}<select value={selected?.key ?? ""} onChange={e => {
        const attachment = info.data.attachments.find(item => item.key === e.target.value);
        if (!attachment) return;
        saveZoteroAttachmentSelection(readingSessionKey, sourceId, space, attachment.key);
        setSelection(previous => ({ attachment: attachment.key, revision: previous.revision + 1 }));
      }}>
        {info.data.attachments.map(a => <option key={a.key} value={a.key}>{a.title || a.key}</option>)}
      </select></label>}
      {selected && !frozen && <button type="button" className="source-pdf-open-zotero" aria-label={t("pdfReader.openZotero")} title={t("pdfReader.openZotero")} onClick={() => void openSearchResult(`zotero://open-pdf/library/items/${encodeURIComponent(selected.key)}`).catch(() => toast(t("sourceAccess.openFailed")))}><ArrowSquareOut size={14} weight="regular" aria-hidden="true" /><span>{t("pdfReader.openZotero")}</span></button>}
    </div>
    {!selected?.available ? <div><p>{t(frozen ? "pdfReader.historicalUnavailable" : "pdfReader.missing")}</p>
        {info.data.registered_source_id && <button type="button" className="page-editor-action" disabled={sync.isPending} onClick={() => sync.mutate(info.data.registered_source_id!)}>{t(sync.isPending ? "pdfReader.syncing" : "pdfReader.retrySync")}</button>}
        {sync.isError && <p role="alert">{t("pdfReader.syncFailed")}</p>}</div>
      : pdf.isError ? <div role="alert"><p>{t(frozen ? "pdfReader.historicalUnavailable" : "pdfReader.failed")}</p><button type="button" className="page-editor-action" onClick={() => void info.refetch().then(() => queryClient.invalidateQueries({ queryKey: ["zotero-pdf", sourceId] }))}>{t("sourceAccess.retry")}</button> {info.data.registered_source_id && <button type="button" className="page-editor-action" disabled={sync.isPending} onClick={() => sync.mutate(info.data.registered_source_id!)}>{t(sync.isPending ? "pdfReader.syncing" : "pdfReader.retrySync")}</button>} {sync.isError && <p>{t("pdfReader.syncFailed")}</p>}</div>
      : !pdf.data ? <p role="status">{t("pdfReader.loading")}</p>
      : <Suspense fallback={<p role="status">{t("pdfReader.loading")}</p>}><PdfReader key={selection.revision} data={pdf.data} documentId={`${sourceId}:${selected.key}`} page={selection.page} readingSessionKey={readingSessionKey} annotations={selected.pdf_digest ? (info.data.annotations ?? []).filter(a => a.attachment_key === selected.key) : []} quote={!citationWarning && citation?.pdf_target?.attachment_key === selected.key && citation.pdf_target.page === selection.page ? citation.pdf_target.quote : undefined} /></Suspense>}
  </section>;
}
