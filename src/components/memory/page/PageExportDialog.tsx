import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { useTranslation } from "react-i18next";
import { capturePageExport, getPageExportBundle, listPageExportVersions, type PageExportBundle, type PageExportVersion } from "../../../lib/pageExportApi";
import { pageExportFilename, serializePageExport, type PageExportFormat, type PageExportSnapshot } from "../../../lib/pageExport";
import SourceDialog from "../sources/SourceDialog";

function snapshot(bundle: PageExportBundle): PageExportSnapshot {
  return { page: { id: bundle.page_id, title: bundle.title, content: bundle.content, version: bundle.version, last_modified: bundle.last_modified, citations: bundle.citations ?? undefined }, kind: bundle.kind, bibliographyIncomplete: bundle.bibliography.some(entry => entry.availability !== "available" || !entry.bibtex || !/@\w+\s*\{/u.test(entry.bibtex)), sources: bundle.sources, zoteroDocuments: bundle.bibliography.filter(entry => entry.availability === "available" && entry.bibtex !== null).map(entry => ({ sourceId: entry.source_id, title: entry.title, bibtex: entry.bibtex! })) };
}

export default function PageExportDialog({ pageId, onClose }: { pageId: string; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const [format, setFormat] = useState<PageExportFormat>("markdown");
  const [selection, setSelection] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current = useQuery({ queryKey: ["page-export-capture", pageId], queryFn: () => capturePageExport(pageId), retry: false, staleTime: 0, refetchOnMount: "always", refetchOnWindowFocus: false });
  const versions = useQuery({ queryKey: ["page-export-versions", pageId, current.data?.id], queryFn: () => listPageExportVersions(pageId), enabled: current.isSuccess && !current.isFetching, retry: false, staleTime: 0, refetchOnMount: "always" });
  const selected = selection ?? current.data?.id ?? "";
  const listed = versions.data?.versions.some(item => item.bundle_id === selected && item.references_available) ?? false;
  const listReady = current.isSuccess && !current.isFetching && versions.isSuccess && !versions.isFetching;
  const bundleQuery = useQuery({ queryKey: ["page-export-bundle", pageId, selected], queryFn: () => getPageExportBundle(pageId, selected), enabled: listReady && listed, retry: false, staleTime: 0, refetchOnMount: "always" });
  const bundle = bundleQuery.data;
  const bibtexUnavailable = !bundle || !bundle.bibliography.length || bundle.bibliography.some(entry => entry.availability !== "available" || !entry.bibtex || !/@\w+\s*\{/u.test(entry.bibtex));
  const canExport = listReady && listed && bundleQuery.isSuccess && !bundleQuery.isFetching && !!bundle && bundle.id === selected && bundle.page_id === pageId && bundle.references_available && !saving && (format !== "bibtex" || !bibtexUnavailable);
  const label = (entry: PageExportVersion) => {
    const date = entry.captured_at ? new Intl.DateTimeFormat(i18n.language, { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit" }).format(new Date(entry.captured_at)) : entry.saved_at;
    return t(entry.kind === "pending_draft" ? "pageExport.pendingVersion" : entry.kind === "legacy_unavailable" ? "pageExport.legacyVersion" : "pageExport.version", { version: entry.version, date });
  };
  async function saveSnapshot() {
    if (!bundle || !canExport) return;
    setError(null); setSaved(false); setSaving(true);
    try {
      const filename = pageExportFilename(bundle.kind === "pending_draft" ? `${bundle.title}-pending-draft` : bundle.title, format);
      const selectedPath = await save({ defaultPath: filename, filters: [{ name: t(`pageExport.${format}`), extensions: [format === "markdown" ? "md" : format === "html" ? "html" : "bib"] }] });
      if (!selectedPath) return;
      const authorized = await getPageExportBundle(pageId, bundle.id);
      if (authorized.id !== bundle.id || authorized.digest !== bundle.digest || authorized.page_id !== pageId || !authorized.references_available) throw new Error(t("pageExport.changed"));
      await writeTextFile(selectedPath, serializePageExport(snapshot(authorized), format));
      setSaved(true);
    } catch (cause) { setError(cause instanceof Error ? cause.message : t("pageExport.saveFailed")); }
    finally { setSaving(false); }
  }
  return <SourceDialog title={t("pageExport.title")} onClose={onClose}>
    <p>{t("pageExport.description")}</p>
    {(current.isFetching || current.isSuccess && versions.isFetching) && <p role="status">{t("pageExport.loading")}</p>}
    {(current.isError || versions.isError) && <p role="alert">{t("pageExport.failed")} <button type="button" onClick={() => void (current.isError ? current.refetch() : versions.refetch())}>{t("sourceAccess.retry")}</button></p>}
    {listReady && <><label>{t("pageExport.chooseVersion")} <select aria-label={t("pageExport.chooseVersion")} value={selected} disabled={saving} onChange={event => { setSelection(event.target.value); setSaved(false); setError(null); }}>
      {!selected && <option value="">{t("pageExport.noVersion")}</option>}
      {versions.data.versions.map((entry, index) => <option key={entry.bundle_id ?? `legacy-${index}`} value={entry.bundle_id ?? `legacy-${index}`} disabled={!entry.bundle_id || !entry.references_available}>{label(entry)}{!entry.references_available ? ` — ${t("pageExport.referencesUnavailable")}` : ""}</option>)}
    </select></label><p>{t("pageExport.capturedNow")}</p></>}
    {bundleQuery.isPending && !!selected && <p role="status">{t("pageExport.loading")}</p>}
    {bundleQuery.isError && <p role="alert">{t("pageExport.failed")} <button type="button" onClick={() => void bundleQuery.refetch()}>{t("sourceAccess.retry")}</button></p>}
    {bundle && !bundle.references_available && <p role="alert">{t("pageExport.referencesUnavailable")}</p>}
    {bundle && bundle.bibliography.some(entry => entry.availability !== "available" || !entry.bibtex) && <p role="status">{t("pageExport.partialBibliography")}</p>}
    {bundle && !bundle.bibliography.length && <p role="status">{t("pageExport.bibtexEmpty")}</p>}
    <fieldset className="source-follow-choice"><legend>{t("pageExport.format")}</legend>{(["markdown", "html", "bibtex"] as PageExportFormat[]).map(value => <label key={value}><input type="radio" name="page-export-format" value={value} checked={format === value} disabled={saving || value === "bibtex" && bibtexUnavailable} onChange={() => { setFormat(value); setSaved(false); }} />{t(`pageExport.${value}`)}</label>)}</fieldset>
    {saved && <p role="status">{t("pageExport.saved")}</p>}{error && <p role="alert">{t("pageExport.saveFailed")} {error}</p>}
    <div className="source-dialog-actions"><button type="button" className="page-editor-action" onClick={onClose}>{t("pageExport.cancel")}</button><button type="button" className="page-editor-action" onClick={() => void saveSnapshot()} disabled={!canExport}>{saving ? t("pageExport.saving") : t("pageExport.save")}</button></div>
  </SourceDialog>;
}
