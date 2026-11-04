import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { useTranslation } from "react-i18next";
import { getPage, getPageSources, getZoteroDocument } from "../../../lib/tauri";
import { pageExportFilename, serializePageExport, type PageExportFormat, type PageExportSnapshot, type ZoteroBibRecord } from "../../../lib/pageExport";
import SourceDialog from "../sources/SourceDialog";

export default function PageExportDialog({ pageId, onClose }: { pageId: string; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const [format, setFormat] = useState<PageExportFormat>("markdown");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const pageQuery = useQuery({ queryKey: ["page-export", pageId], queryFn: () => getPage(pageId), retry: false, staleTime: 0, refetchOnMount: "always" });
  const page = pageQuery.data;
  const sourcesQuery = useQuery({ queryKey: ["page-export-sources", pageId], queryFn: () => getPageSources(pageId), enabled: pageQuery.isSuccess && page !== null, retry: false });
  const zoteroSources = (sourcesQuery.data ?? []).filter(row => row.memory?.source_agent === "zotero" || row.memory?.source_id.startsWith("zotero::") || row.source.memory_source_id.startsWith("zotero::"));
  const bibliographyQuery = useQuery({
    queryKey: ["page-export-bibtex", pageId, page?.space ?? null, zoteroSources.map(row => row.memory?.source_id ?? row.source.memory_source_id)],
    queryFn: async () => {
      const results = await Promise.all(zoteroSources.map(async row => {
        const sourceId = row.memory?.source_id ?? row.source.memory_source_id;
        try {
          const doc = await getZoteroDocument(sourceId, page?.space ?? undefined);
          return { sourceId, record: { sourceId, title: doc.title || row.memory?.title || "", bibtex: doc.bibtex } satisfies ZoteroBibRecord };
        } catch {
          return { sourceId, record: null };
        }
      }));
      return {
        records: results.flatMap(result => result.record ? [result.record] : []),
        unavailableSourceIds: results.filter(result => !result.record).map(result => result.sourceId),
      };
    },
    enabled: sourcesQuery.isSuccess && zoteroSources.length > 0,
    retry: false,
  });
  const snapshot = useMemo<PageExportSnapshot | null>(() => page && sourcesQuery.data
    ? { page, sources: sourcesQuery.data, zoteroDocuments: bibliographyQuery.data?.records ?? [] }
    : null, [page, sourcesQuery.data, bibliographyQuery.data?.records]);
  const bibliographyEntries = (bibliographyQuery.data?.records ?? []).filter(record => /@\w+\s*\{/u.test(record.bibtex)).length;
  const hasUnavailableBibliography = bibliographyQuery.isError
    || (bibliographyQuery.data?.unavailableSourceIds.length ?? 0) > 0
    || bibliographyQuery.data !== undefined && bibliographyEntries < zoteroSources.length;
  const bibtexUnavailable = !zoteroSources.length || hasUnavailableBibliography || bibliographyQuery.isSuccess && bibliographyEntries === 0;
  const noBibliography = sourcesQuery.isSuccess && (!zoteroSources.length || bibliographyQuery.isSuccess && bibliographyEntries === 0 && !hasUnavailableBibliography);
  const bibtexLoading = zoteroSources.length > 0 && bibliographyQuery.isPending;
  const canExport = !!snapshot && !pageQuery.isError && sourcesQuery.isSuccess && !saving && !bibtexLoading && (format !== "bibtex" || !bibtexUnavailable);

  async function saveSnapshot() {
    if (!snapshot || !canExport) return;
    setSaveError(null); setSaved(false); setSaving(true);
    try {
      const filename = pageExportFilename(snapshot.page.title, format);
      const selectedPath = await save({ defaultPath: filename, filters: [{ name: t(`pageExport.${format}`), extensions: [format === "markdown" ? "md" : format === "html" ? "html" : "bib"] }] });
      if (!selectedPath) return;
      // Optimistically recheck the captured inputs after the potentially long
      // native dialog. Changes trigger a retry; this is not a DB transaction.
      const capturedZoteroSources = snapshot.sources.filter(row => row.memory?.source_agent === "zotero" || row.memory?.source_id.startsWith("zotero::") || row.source.memory_source_id.startsWith("zotero::"));
      const bibliographyCheck = await (async () => {
        const results = await Promise.all(capturedZoteroSources.map(async row => {
          const sourceId = row.memory?.source_id ?? row.source.memory_source_id;
          try {
            const doc = await getZoteroDocument(sourceId, page?.space ?? undefined);
            return { sourceId, record: { sourceId, title: doc.title || row.memory?.title || "", bibtex: doc.bibtex } satisfies ZoteroBibRecord };
          } catch {
            return { sourceId, record: null };
          }
        }));
        return {
          records: results.flatMap(result => result.record ? [result.record] : []),
          unavailableSourceIds: results.filter(result => !result.record).map(result => result.sourceId),
        };
      })();
      const [currentPage, currentSources, currentBibliography] = await Promise.all([
        getPage(pageId), getPageSources(pageId),
        Promise.resolve(bibliographyCheck),
      ]);
      if (!currentPage || currentPage.version !== snapshot.page.version || currentPage.content !== snapshot.page.content
        || JSON.stringify(currentPage.citations) !== JSON.stringify(snapshot.page.citations)
        || JSON.stringify(currentSources) !== JSON.stringify(snapshot.sources)
        || JSON.stringify(currentBibliography.records) !== JSON.stringify(snapshot.zoteroDocuments)
        || JSON.stringify(currentBibliography.unavailableSourceIds) !== JSON.stringify(bibliographyQuery.data?.unavailableSourceIds ?? [])) {
        await Promise.all([pageQuery.refetch(), sourcesQuery.refetch(), bibliographyQuery.refetch()]);
        throw new Error(t("pageExport.changed"));
      }
      const contents = serializePageExport(snapshot, format);
      await writeTextFile(selectedPath, contents);
      setSaved(true);
    } catch (error) {
      setSaveError(error instanceof Error ? error.message : t("pageExport.saveFailed"));
    } finally { setSaving(false); }
  }

  const modified = page?.last_modified ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(page.last_modified)) : "";
  const formats: PageExportFormat[] = ["markdown", "html", "bibtex"];
  return <SourceDialog title={t("pageExport.title")} onClose={onClose}>
    <p>{t("pageExport.description")}</p>
    {pageQuery.isPending && <p role="status">{t("pageExport.loading")}</p>}
    {pageQuery.isError && <p role="alert">{t("pageExport.failed")}</p>}
    {pageQuery.isSuccess && !page && <p role="alert">{t("pageExport.failed")}</p>}
    {page && <p className="source-dialog-status" role="status">{t("pageExport.version", { version: page.version, date: modified })}</p>}
    {sourcesQuery.isPending && page && <p role="status">{t("pageExport.loading")}</p>}
    {sourcesQuery.isError && <p role="alert">{t("pageExport.sourcesFailed")} <button type="button" className="page-editor-action" onClick={() => void sourcesQuery.refetch()}>{t("sourceAccess.retry")}</button></p>}
    {bibtexLoading && <p role="status">{t("pageExport.bibtexLoading")}</p>}
    {hasUnavailableBibliography && <p role="alert">{t("pageExport.bibtexFailed")} <button type="button" className="page-editor-action" onClick={() => void bibliographyQuery.refetch()}>{t("sourceAccess.retry")}</button></p>}
    {noBibliography && <p role="status">{t("pageExport.bibtexEmpty")}</p>}
    <fieldset className="source-follow-choice">
      <legend>{t("pageExport.format")}</legend>
      {formats.map(value => <label key={value}><input type="radio" name="page-export-format" value={value} checked={format === value}
        disabled={value === "bibtex" && (bibtexUnavailable || bibtexLoading)} onChange={() => { setFormat(value); setSaved(false); }} />{t(`pageExport.${value}`)}</label>)}
    </fieldset>
    {saved && <p role="status">{t("pageExport.saved")}</p>}
    {saveError && <p role="alert">{t("pageExport.saveFailed")} {saveError}</p>}
    <div className="source-dialog-actions">
      <button type="button" className="page-editor-action" onClick={onClose}>{t("pageExport.cancel")}</button>
      <button type="button" className="page-editor-action" onClick={() => void saveSnapshot()} disabled={!canExport}>{saving ? t("pageExport.saving") : t("pageExport.save")}</button>
    </div>
  </SourceDialog>;
}
