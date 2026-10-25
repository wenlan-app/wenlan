// SPDX-License-Identifier: AGPL-3.0-only
import { SourceImportProgress } from "./SourceImportProgress";
import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Funnel, Plus } from "@phosphor-icons/react";
import { listIndexedFiles, listRegisteredSources, type IndexedFileInfo } from "../../../lib/tauri";
import type { AssetLens } from "../../../lib/assetLens";
import { getSourceAppFromUrl, getSourcePresentation, isNoteApp, isSourceApp, SOURCE_APP_NAMES, sourceAppSearchNames, type SourceApp } from "./sourcePresentation";
import { SourceTypeIcon } from "./SourceTypeIcon";
import { SourceCollection, SourceCollectionToolbar, type SourceCollectionItem } from "./SourceCollection";
import SourceContainer, { type SourceContainerLocation } from "./SourceContainer";
import "../assets/assetCards.css";
import "../assets/collectionToolbar.css";
import "./SourceLibrary.css";

/** An app filter lists only that app's saved sources; "links" keeps the ordinary web pages. */
export type SourceLibraryFilter = "all" | "files" | "links" | "folders" | SourceApp;

export interface SourceLibraryState {
  search: string;
  filter: SourceLibraryFilter;
  lens?: AssetLens;
  focusedDocument?: string;
  container?: SourceContainerLocation | null;
}

export interface SourceLibraryProps {
  onAdd: () => void;
  onOpenDocument: (file: IndexedFileInfo) => void;
  /** Provide both props to retain search and filter in the owning route. */
  state?: SourceLibraryState;
  onStateChange?: (state: SourceLibraryState) => void;
}

function normalizedPath(path: string): string {
  return path.replace(/\\/g, "/").replace(/\/+$/, "");
}

function pathName(path: string): string {
  return normalizedPath(path).split("/").pop() || path;
}

function documentPath(file: IndexedFileInfo): string {
  const separator = file.source_id.indexOf("::");
  return separator >= 0 ? file.source_id.slice(separator + 2) : file.source_id;
}

/** Only imported document sources belong here, even when a memory has a URL. */
export function isLibraryDocument(file: IndexedFileInfo): boolean {
  if (file.memory_type === "recap") return false;
  return ["file", "webpage", "directory", "obsidian"].includes(file.source) ||
    (file.source === "memory" && (file.source_agent === "folder" || isNoteApp(file.source_agent)));
}

function documentUrl(file: IndexedFileInfo): URL | null {
  for (const candidate of [file.source === "webpage" ? file.source_id : file.url]) {
    if (!candidate) continue;
    try {
      const url = new URL(candidate);
      if (url.protocol === "http:" || url.protocol === "https:") return url;
    } catch { /* Source IDs and local paths are not URLs. */ }
  }
  return null;
}

export default function SourceLibrary({ onAdd, onOpenDocument, state, onStateChange }: SourceLibraryProps) {
  const { t, i18n } = useTranslation();
  const libraryRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      const button = [...(libraryRef.current?.querySelectorAll<HTMLButtonElement>("[data-source-key]") ?? [])]
        .find(element => element.dataset.sourceKey === state?.focusedDocument);
      button?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, []);
  const [localState, setLocalState] = useState<SourceLibraryState>({ search: "", filter: "all" });
  const currentState = state ?? localState;
  const lens = currentState.lens ?? "rows";
  const { search, filter } = currentState;
  const foldersQuery = useQuery({ queryKey: ["registeredSources"], queryFn: listRegisteredSources });
  const filesQuery = useQuery({ queryKey: ["indexedFiles"], queryFn: () => listIndexedFiles(), refetchInterval: 5000 });

  const folders = useMemo(() => (foldersQuery.data ?? [])
    .filter((source) => !/(?:^|\/)\.wenlan\/sources$/.test(normalizedPath(source.path)))
    .sort((a, b) => pathName(a.path).localeCompare(pathName(b.path))), [foldersQuery.data]);
  const documents = useMemo(() => (filesQuery.data ?? []).filter(isLibraryDocument)
    .sort((a, b) => a.title.localeCompare(b.title)), [filesQuery.data]);
  const needle = search.trim().toLocaleLowerCase();
  const appOf = (file: IndexedFileInfo) => getSourceAppFromUrl(file.url ?? file.source_id);
  const savedApps = useMemo(() => {
    const present = new Set(documents.map(appOf).filter((app): app is SourceApp => app !== null));
    if (isSourceApp(filter)) present.add(filter);
    return (Object.keys(SOURCE_APP_NAMES) as SourceApp[]).filter((app) => present.has(app));
  }, [documents, filter]);
  const visibleFolders = filter !== "all" && filter !== "folders" ? [] : folders.filter((source) =>
    `${pathName(source.path)} ${source.path}`.toLocaleLowerCase().includes(needle));
  const coveredDocuments = useMemo(() => new Set(documents.filter((file) => folders.some((source) => {
    const path = normalizedPath(documentPath(file));
    const root = normalizedPath(source.path);
    return file.source_id.toLocaleLowerCase().startsWith(`${source.id}::`.toLocaleLowerCase()) && path.toLocaleLowerCase().startsWith(`${root.toLocaleLowerCase()}/`);
  })).map((file) => `${file.source}:${file.source_id}`)), [documents, folders]);
  const visibleDocuments = filter === "folders" ? [] : documents.filter((file) => {
    const link = file.source === "webpage" || documentUrl(file) !== null;
    const app = appOf(file);
    const visibleType = filter === "all" ? (!!needle || !coveredDocuments.has(`${file.source}:${file.source_id}`))
      : filter === "links" ? link && !app : filter === "files" ? !link : app === filter;
    return visibleType &&
      `${file.title} ${documentPath(file)} ${file.url ?? ""} ${app ? sourceAppSearchNames(app) : ""}`.toLocaleLowerCase().includes(needle);
  });
  const bothLoaded = foldersQuery.isSuccess && filesQuery.isSuccess;
  const realEmpty = bothLoaded && folders.length === 0 && documents.length === 0;
  const filteredEmpty = bothLoaded && !realEmpty && visibleFolders.length === 0 && visibleDocuments.length === 0;

  function updateState(next: Partial<SourceLibraryState>) {
    const updated = { ...(state ?? localState), ...next };
    if (state === undefined) setLocalState(updated);
    onStateChange?.(updated);
  }

  function clearFilters() { updateState({ search: "", filter: "all" }); }

  if (currentState.container) return <SourceContainer
    location={currentState.container}
    search={search}
    filter={filter}
    lens={lens}
    onSearchChange={(value) => updateState({ search: value })}
    onFilterChange={(value) => updateState({ filter: value })}
    onLensChange={(value) => updateState({ lens: value })}
    onClearFilters={clearFilters}
    onNavigate={(container) => updateState({ container, search: "", filter: "all" })}
    onAdd={onAdd}
    onOpenDocument={onOpenDocument}
  />;

  const collectionItems: SourceCollectionItem[] = [
    ...visibleFolders.map((source) => ({
      id: `container:${source.id}`,
      title: pathName(source.path),
      metadata: `${source.source_type === "obsidian" ? t("sourceContainer.obsidianVault") : source.source_type === "logseq" ? t("sourceContainer.logseqGraph") : t("sourceLibrary.folder")} · ${source.path}`,
      kind: isNoteApp(source.source_type) ? source.source_type : "folder" as const,
      onSelect: () => updateState({ search: "", filter: "all", container: { sourceId: source.id, path: source.path } }),
    })),
    ...visibleDocuments.map((file) => {
      const path = normalizedPath(documentPath(file));
      const presentation = getSourcePresentation(file, { document: t("sourceLibrary.document"), webExcerpt: t("sourceLibrary.webExcerpt"), language: i18n.language });
      return {
        id: `${file.source}:${file.source_id}`,
        title: file.title || pathName(path),
        metadata: presentation.metadata,
        kind: presentation.iconKind,
        onSelect: () => {
          updateState({ focusedDocument: `${file.source}:${file.source_id}` });
          onOpenDocument(file);
        },
      };
    }),
  ];

  return (
    <section ref={libraryRef} className="source-library" aria-labelledby="source-library-title">
      <h1 className="sr-only" id="source-library-title">{t("sourceLibrary.title")}</h1>

      <SourceCollectionToolbar search={search} filter={filter} lens={lens} apps={savedApps}
        onSearchChange={(value) => updateState({ search: value })}
        onFilterChange={(value) => updateState({ filter: value })}
        onLensChange={(value) => updateState({ lens: value })} onAdd={onAdd} />

      <SourceImportProgress />

      <div className="source-library-notices">
        {foldersQuery.isPending && <p role="status">{t("sourceLibrary.loadingFolders")}</p>}
        {filesQuery.isPending && <p role="status">{t("sourceLibrary.loadingDocuments")}</p>}
        {foldersQuery.isError && <div role="alert"><span>{t("sourceLibrary.folderError")}</span><button onClick={() => void foldersQuery.refetch()}>{t("sourceLibrary.retryFolders")}</button></div>}
        {filesQuery.isError && <div role="alert"><span>{t("sourceLibrary.documentError")}</span><button onClick={() => void filesQuery.refetch()}>{t("sourceLibrary.retryDocuments")}</button></div>}
      </div>

      {collectionItems.length > 0 && <SourceCollection items={collectionItems} lens={lens} label={t("sourceLibrary.listLabel")} />}

      {realEmpty && <div className="source-library-empty"><SourceTypeIcon kind="file" size={32} /><h2>{t("sourceLibrary.emptyTitle")}</h2><p>{t("sourceLibrary.emptyBody")}</p><p className="source-library-support">{t("sourceLibrary.supportedFiles")}</p><button className="source-library-add" onClick={onAdd}><Plus size={16} aria-hidden="true" />{t("sourceLibrary.add")}</button></div>}
      {filteredEmpty && <div className="source-library-empty"><Funnel size={28} aria-hidden="true" /><h2>{t("sourceLibrary.noMatches")}</h2><p>{t("sourceLibrary.noMatchesBody")}</p><button className="source-library-manage" onClick={clearFilters}>{t("sourceLibrary.clearFilters")}</button></div>}
    </section>
  );
}
