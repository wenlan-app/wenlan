// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listAllTags, listIndexedFiles, type IndexedFileInfo } from "../../../lib/tauri";
import type { AssetLens } from "../../../lib/assetLens";
import { isLibraryDocument } from "../sources/SourceLibrary";
import SourceDocumentPreview from "../sources/SourceDocumentPreview";
import { getSourcePresentation } from "../sources/sourcePresentation";
import { SourceTypeIcon } from "../sources/SourceTypeIcon";
import "../sources/SourceLibrary.css";
import ZoteroSourceDialog from "../sources/ZoteroSourceDialog";
import { SelectedSourceActions } from "../sources/SelectedSourceActions";
import { documentTagKey } from "../sources/sourceOrganize";
import { readSourceSelection, writeSourceSelection } from "./sourceSelectionState";

export function SpaceSources({ spaceId, spaceName, lens, onOpenSource, onRegisterPreviewFlush }: { spaceId: string; spaceName: string; lens: AssetLens; onOpenSource?: (file: IndexedFileInfo) => void; onRegisterPreviewFlush?: (flush: (() => Promise<boolean>) | null) => void }) {
  const { t, i18n } = useTranslation();
  const [selected, setSelected] = useState<IndexedFileInfo | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [selectedIds, setSelectedIds] = useState<string[]>(() => readSourceSelection(spaceId));
  const [tagFilter, setTagFilter] = useState("");
  const [action, setAction] = useState<"organize" | "tags" | null>(null);
  const [addZotero, setAddZotero] = useState(false);
  const client = useQueryClient();
  const triggerRef = useRef<HTMLElement | null>(null);
  const closePreview = () => {
    const trigger = triggerRef.current;
    triggerRef.current = null;
    setSelected(null);
    if (!trigger) return;
    requestAnimationFrame(() => {
      if (trigger.isConnected) trigger.focus();
    });
  };
  const query = useQuery({ queryKey: ["space-sources", spaceId, spaceName], queryFn: () => listIndexedFiles(spaceName), retry: false });
  const tagData = useQuery({ queryKey: ["tags"], queryFn: listAllTags, retry: false });
  const documents = (query.data ?? []).filter(isLibraryDocument).sort((a, b) => a.title.localeCompare(b.title));
  useEffect(() => {
    if (query.isPending || query.isError) return;
    const present = new Set(documents.map((file) => `${file.source}::${file.source_id}`));
    setSelectedIds((ids) => ids.filter((id) => present.has(id)));
  }, [query.data, query.isPending, query.isError]);
  useEffect(() => writeSourceSelection(spaceId, selectedIds), [spaceId, selectedIds]);
  const chosen = documents.filter((file) => selectedIds.includes(`${file.source}::${file.source_id}`));
  const visibleDocuments = tagFilter ? documents.filter((file) => (tagData.data?.document_tags[documentTagKey(file.source, file.source_id)] ?? []).includes(tagFilter)) : documents;
  const visibleIds = visibleDocuments.map((file) => `${file.source}::${file.source_id}`);
  const allVisibleSelected = visibleIds.length > 0 && visibleIds.every((id) => selectedIds.includes(id));
  const toggleVisibleSelection = () => setSelectedIds((ids) => allVisibleSelected ? ids.filter((id) => !visibleIds.includes(id)) : [...new Set([...ids, ...visibleIds])].slice(0, 100));
  return <>
    <div className="flex justify-end gap-2 mb-2"><button type="button" className="page-editor-action" onClick={() => { setSelecting((value) => !value); if (selecting) setSelectedIds([]); }}>{selecting ? t("selectedSources.cancelSelect") : t("selectedSources.select")}</button><button type="button" className="page-editor-action" onClick={() => setAddZotero(true)}>{t("zoteroSource.addToSpace")}</button></div>
    <div className="flex items-center gap-2 mb-2"><label>{t("selectedSources.filterByTag")} <select aria-label={t("selectedSources.filterByTag")} value={tagFilter} onChange={(event) => setTagFilter(event.target.value)}><option value="">{t("selectedSources.allTags")}</option>{(tagData.data?.tags ?? []).map((tag) => <option key={tag} value={tag}>{tag}</option>)}</select></label>{selecting ? <button type="button" className="page-editor-action" onClick={toggleVisibleSelection}>{allVisibleSelected ? t("selectedSources.clearVisible") : t("selectedSources.selectVisible")}</button> : null}</div>
    {selecting && chosen.length ? <p role="status">{t("selectedSources.savedSelection", { count: chosen.length, visible: chosen.filter((file) => visibleIds.includes(`${file.source}::${file.source_id}`)).length })}</p> : null}
    {selecting && chosen.length > 0 ? <div className="flex gap-2 mb-2"><button type="button" className="page-editor-action" onClick={() => setAction("organize")}>{t("selectedSources.organize", { count: chosen.length })}</button><button type="button" className="page-editor-action" onClick={() => setAction("tags")}>{t("selectedSources.editTags", { count: chosen.length })}</button></div> : null}
    {query.isPending ? <p role="status" className="space-dossier-empty">{t("sourceAccess.previewLoading")}</p> : query.isError ? <div role="alert"><p className="space-dossier-empty">{t("spaceDetail.sourcesFailed")}</p><button className="page-editor-action" type="button" onClick={() => void query.refetch()}>{t("sourceAccess.retry")}</button></div> : <>
    {documents.length === 0 ? <p className="space-dossier-empty">{t("spaceDetail.noSources")}</p> : visibleDocuments.length === 0 ? <p className="space-dossier-empty">{t("selectedSources.noTagMatches")}</p> : <ul className="source-library-list space-project-sources" data-lens={lens}>
      {visibleDocuments.map(file => {
        const name = file.source_id.split("::").pop()?.replace(/\\/g, "/").split("/").pop() ?? file.title;
        const tags = tagData.data?.document_tags[documentTagKey(file.source, file.source_id)] ?? [];
        const presentation = getSourcePresentation(file, {
          document: t("sourceLibrary.document"),
          webExcerpt: t("sourceLibrary.webExcerpt"),
          language: i18n.language,
        });
        const identity = `${file.source}::${file.source_id}`;
        return <li key={identity}>{selecting ? <label className="flex items-center gap-2"><input type="checkbox" aria-label={t("selectedSources.selectSource", { title: file.title })} checked={selectedIds.includes(identity)} disabled={selectedIds.length >= 100 && !selectedIds.includes(identity)} onChange={(event) => setSelectedIds((ids) => event.target.checked ? [...new Set([...ids, identity])].slice(0, 100) : ids.filter((id) => id !== identity))} /><span className="sr-only">{t("selectedSources.selectSource", { title: file.title })}</span></label> : null}<button type="button" className="source-library-row" onClick={(event) => { if (selecting) { setSelectedIds((ids) => ids.includes(identity) ? ids.filter((id) => id !== identity) : [...new Set([...ids, identity])].slice(0, 100)); return; } triggerRef.current = event.currentTarget; setSelected(file); }}>
          <span className="source-library-icon"><SourceTypeIcon kind={presentation.iconKind} /></span>
          <span className="source-library-row-content"><span className="source-library-row-title">{file.title || name}</span><span className="source-library-row-meta">{presentation.metadata}</span>{tags.length ? <span className="flex flex-wrap gap-1">{tags.map((tag) => <span key={tag} className="memory-chip indigo">{tag}</span>)}</span> : null}</span>
        </button></li>;
      })}
    </ul>}
    </>}
    {selected && <SourceDocumentPreview key={`${spaceId}:${spaceName}:${selected.source}:${selected.source_id}`} space={spaceName} file={selected} onClose={closePreview} onRegisterFlush={onRegisterPreviewFlush} onOpenSource={onOpenSource ? () => onOpenSource({ ...selected, space: spaceName }) : undefined} />}
    {action && chosen.length > 0 && <SelectedSourceActions spaceId={spaceId} spaceName={spaceName} sources={chosen} mode={action} onClose={() => setAction(null)} />}
    {addZotero && <ZoteroSourceDialog fixedSpace={spaceName} onClose={() => { setAddZotero(false); void client.invalidateQueries({ queryKey: ["space-sources", spaceId, spaceName] }); }} />}
  </>;
}
