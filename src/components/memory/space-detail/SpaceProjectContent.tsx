// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useId, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import type { IndexedFileInfo, Page, SharedStatus } from "../../../lib/tauri";
import type { AssetLens } from "../../../lib/assetLens";
import { AssetLensToggle } from "../assets/AssetLensToggle";
import "../assets/assetCards.css";
import "../assets/collectionToolbar.css";
import type { SpaceDetailCopy } from "./copy";
import { SpaceDossierContent } from "./SpaceDossierContent";
import { SpaceSources } from "./SpaceSources";

type Props = {
  copy: SpaceDetailCopy;
  spaceId: string;
  spaceName: string;
  pages: readonly Page[];
  pagesPending: boolean;
  pagesError: boolean;
  onRetryPages: () => void;
  onSelectPage: (id: string) => void;
  onOpenSource?: (file: IndexedFileInfo) => void;
  onRegisterPreviewFlush?: (flush: (() => Promise<boolean>) | null) => void;
  /** Everything shared, so a row can say who else changed the page. */
  shares?: readonly SharedStatus[];
};

export function SpaceProjectContent({ copy, spaceId, spaceName, pages, pagesPending, pagesError, onRetryPages, onSelectPage, onOpenSource, onRegisterPreviewFlush, shares }: Props) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"notes" | "sources">("notes");
  const [lenses, setLenses] = useState<Readonly<Record<string, AssetLens>>>({});
  const lens = lenses[spaceId] ?? "rows";
  const sourcePreviewFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const registerSourcePreviewFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    sourcePreviewFlushRef.current = flush;
  }, []);
  const registerComposedPreviewFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    registerSourcePreviewFlush(flush);
    onRegisterPreviewFlush?.(flush);
  }, [onRegisterPreviewFlush, registerSourcePreviewFlush]);
  const setLens = (next: AssetLens) => setLenses((current) => ({ ...current, [spaceId]: next }));
  const id = useId();
  const notesRef = useRef<HTMLButtonElement>(null);
  const sourcesRef = useRef<HTMLButtonElement>(null);
  async function selectTab(next: "notes" | "sources") {
    if (tab === "sources" && next !== "sources" && sourcePreviewFlushRef.current && !await sourcePreviewFlushRef.current()) return;
    setTab(next);
  }
  function changeTab(event: KeyboardEvent<HTMLDivElement>) {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const next = event.key === "Home" ? "notes" : event.key === "End" ? "sources" : tab === "notes" ? "sources" : "notes";
    void selectTab(next);
    (next === "notes" ? notesRef : sourcesRef).current?.focus();
  }
  return <>
    <div className="space-project-controls collection-toolbar">
      <div className="space-project-tabs" role="tablist" aria-label={t("spaceDetail.projectContent")} onKeyDown={changeTab}>
        <button ref={notesRef} id={`${id}-notes`} aria-controls={`${id}-panel`} role="tab" aria-selected={tab === "notes"} tabIndex={tab === "notes" ? 0 : -1} type="button" onClick={() => { void selectTab("notes"); }}>{t("spaceDetail.notesTab")}</button>
        <button ref={sourcesRef} id={`${id}-sources`} aria-controls={`${id}-panel`} role="tab" aria-selected={tab === "sources"} tabIndex={tab === "sources" ? 0 : -1} type="button" onClick={() => { void selectTab("sources"); }}>{t("spaceDetail.sourcesTab")}</button>
      </div>
      <AssetLensToggle value={lens} onChange={setLens} />
    </div>
    <div id={`${id}-panel`} role="tabpanel" aria-labelledby={`${id}-${tab}`}>
      {tab === "sources" ? <SpaceSources key={`${spaceId}:${spaceName}`} spaceId={spaceId} spaceName={spaceName} lens={lens} onOpenSource={onOpenSource} onRegisterPreviewFlush={registerComposedPreviewFlush} /> : pagesPending ? <p role="status" className="space-dossier-empty">{copy.loading}</p> : pagesError ? <div role="alert"><p className="space-dossier-empty">{copy.relatedLoadError}</p><button type="button" className="page-editor-action" onClick={onRetryPages}>{t("sourceAccess.retry")}</button></div> : <SpaceDossierContent copy={copy} pages={pages} lens={lens} navigation={{ onSelectPage }} shares={shares} />}
    </div>
  </>;
}
