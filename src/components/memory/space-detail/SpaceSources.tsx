// SPDX-License-Identifier: AGPL-3.0-only
import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listIndexedFiles, type IndexedFileInfo } from "../../../lib/tauri";
import type { AssetLens } from "../../../lib/assetLens";
import { isLibraryDocument } from "../sources/SourceLibrary";
import SourceDocumentPreview from "../sources/SourceDocumentPreview";
import { getSourcePresentation } from "../sources/sourcePresentation";
import { SourceTypeIcon } from "../sources/SourceTypeIcon";
import "../sources/SourceLibrary.css";

export function SpaceSources({ spaceId, spaceName, lens, onOpenSource, onRegisterPreviewFlush }: { spaceId: string; spaceName: string; lens: AssetLens; onOpenSource?: (file: IndexedFileInfo) => void; onRegisterPreviewFlush?: (flush: (() => Promise<boolean>) | null) => void }) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<IndexedFileInfo | null>(null);
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
  const documents = (query.data ?? []).filter(isLibraryDocument).sort((a, b) => a.title.localeCompare(b.title));
  if (query.isPending) return <p role="status" className="space-dossier-empty">{t("sourceAccess.previewLoading")}</p>;
  if (query.isError) return <div role="alert"><p className="space-dossier-empty">{t("spaceDetail.sourcesFailed")}</p><button className="page-editor-action" type="button" onClick={() => void query.refetch()}>{t("sourceAccess.retry")}</button></div>;
  return <>
    {documents.length === 0 ? <p className="space-dossier-empty">{t("spaceDetail.noSources")}</p> : <ul className="source-library-list space-project-sources" data-lens={lens}>
      {documents.map(file => {
        const name = file.source_id.split("::").pop()?.replace(/\\/g, "/").split("/").pop() ?? file.title;
        const presentation = getSourcePresentation(file, {
          document: t("sourceLibrary.document"),
          webExcerpt: t("sourceLibrary.webExcerpt"),
        });
        return <li key={`${file.source}:${file.source_id}`}><button type="button" className="source-library-row" onClick={(event) => { triggerRef.current = event.currentTarget; setSelected(file); }}>
          <span className="source-library-icon"><SourceTypeIcon kind={presentation.iconKind} /></span>
          <span className="source-library-row-content"><span className="source-library-row-title">{file.title || name}</span><span className="source-library-row-meta">{presentation.metadata}</span></span>
        </button></li>;
      })}
    </ul>}
    {selected && <SourceDocumentPreview key={`${spaceId}:${spaceName}:${selected.source}:${selected.source_id}`} space={spaceName} file={selected} onClose={closePreview} onRegisterFlush={onRegisterPreviewFlush} onOpenSource={onOpenSource ? () => onOpenSource({ ...selected, space: spaceName }) : undefined} />}
  </>;
}
