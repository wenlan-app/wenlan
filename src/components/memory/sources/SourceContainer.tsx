// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CaretRight, ArrowLeft, ArrowClockwise } from "@phosphor-icons/react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  listIndexedFiles,
  listRegisteredSources,
  openFile,
  readSourceDir,
  readSourceText,
  setSourceFollowChanges,
  sourceFollowsChanges,
  syncRegisteredSource,
  type IndexedFileInfo,
  type RegisteredSource,
  type SourceDirEntry,
} from "../../../lib/tauri";
import type { AssetLens } from "../../../lib/assetLens";
import { DetailInfoContent } from "../DetailLayout";
import { CollectionFrame } from "../CollectionLayout";
import { InfoToggle } from "../page/InfoToggle";
import { Toggle } from "../settings/primitives";
import PageInfoDrawer from "../page/PageInfoDrawer";
import { useDetailInfoOpen } from "../useDetailInfoOpen";
import { SourceDocumentLayout } from "./SourceDocumentReader";
import SourceDocumentContent from "./SourceDocumentContent";
import { getSourceAppFromUrl, getSourcePresentation, isNoteApp } from "./sourcePresentation";
import { SourceCollection, SourceCollectionBreadcrumbs, SourceCollectionToolbar, type SourceCollectionItem } from "./SourceCollection";
import { SourceTypeIcon } from "./SourceTypeIcon";
import type { SourceLibraryFilter } from "./SourceLibrary";
import "./SourceContainer.css";

export interface SourceContainerLocation {
  sourceId: string;
  path: string;
}

interface Props {
  location: SourceContainerLocation;
  search: string;
  filter: SourceLibraryFilter;
  lens: AssetLens;
  onSearchChange: (value: string) => void;
  onFilterChange: (value: SourceLibraryFilter) => void;
  onLensChange: (value: AssetLens) => void;
  onClearFilters: () => void;
  onNavigate: (location: SourceContainerLocation | null) => void;
  onAdd: () => void;
  onOpenDocument: (file: IndexedFileInfo) => void;
}

const supportedText = new Set(["md", "markdown", "mdx", "txt", "text"]);

function normalized(path: string) { return path.replace(/\\/g, "/").replace(/\/+$/, ""); }
function basename(path: string) { return normalized(path).split("/").pop() || path; }
function extension(name: string) { return name.toLowerCase().split(".").pop() ?? ""; }
function childPath(parent: string, name: string) { return `${normalized(parent)}/${name}`; }
function documentTitle(name: string) { return name.replace(/\.[^.]+$/, ""); }
function statusLabel(source: RegisteredSource, t: TFunction) {
  if (source.status === "Paused") return t("sourceContainer.paused");
  if (typeof source.status === "object" && "Unavailable" in source.status) return t("sourceContainer.unavailable");
  if (typeof source.status === "object" && "Error" in source.status) return t("sourceContainer.syncError");
  return source.last_sync === null ? t("sourceContainer.notChecked") : t("sourceContainer.active");
}

function formattedDate(ts: number | null, language: string, missing: string) {
  if (!ts) return missing;
  return new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(ts * 1000));
}

function SourceContainerInfo({ source, refreshing, onRefresh, heading }: { source: RegisteredSource; refreshing: boolean; onRefresh: () => void; heading?: string }) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const following = sourceFollowsChanges(source);
  const followMutation = useMutation({
    mutationFn: (followChanges: boolean) => setSourceFollowChanges(source.id, followChanges),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["registeredSources"] }),
    onError: (error) => {
      console.warn("[sources] follow toggle failed", error);
      toast(t("sourceContainer.followFailed"));
    },
  });
  return <>
    <section>
      <h3 className="note-info-section-heading">{heading ?? t("sourceAccess.details")}</h3>
      <dl className="source-info-metadata">
        <div><dt>{t("sourceContainer.sourceType")}</dt><dd>{source.source_type === "obsidian" ? t("sourceContainer.obsidianVault") : source.source_type === "logseq" ? t("sourceContainer.logseqGraph") : t("sourceLibrary.folder")}</dd></div>
        <div><dt>{t("sourceContainer.originalPath")}</dt><dd className="source-container-path">{source.path}</dd></div>
        <div><dt>{t("sourceAccess.status")}</dt><dd>{statusLabel(source, t)}</dd></div>
        <div><dt>{t("sourceContainer.lastChecked")}</dt><dd>{formattedDate(source.last_sync, i18n.language, t("sourceContainer.notChecked"))}</dd></div>
        <div><dt>{t("sourceContainer.files")}</dt><dd>{source.file_count.toLocaleString()}</dd></div>
        <div><dt>{t("sourceContainer.memories")}</dt><dd>{source.memory_count.toLocaleString()}</dd></div>
        {(source.last_sync_errors ?? 0) > 0 && <div><dt>{t("sourceContainer.syncErrors")}</dt><dd>{source.last_sync_errors!.toLocaleString()}</dd></div>}
      </dl>
    </section>
    <div className="source-follow-row">
      <span>{t("sourceContainer.followChanges")}</span>
      <Toggle enabled={following} onToggle={() => followMutation.mutate(!following)}
        valueUnknown={followMutation.isPending} unknownState="saving"
        aria-label={t("sourceContainer.followChanges")} />
    </div>
    <p>{t(following ? "sourceContainer.folderChecks" : "sourceContainer.importedOnce")}</p>
    <p>{t("sourceContainer.oneWay")}</p>
    <button className="collection-control source-container-refresh" type="button" disabled={refreshing} onClick={onRefresh}>
      <ArrowClockwise size={16} aria-hidden="true" />{refreshing ? t("sourceContainer.refreshing") : t("sourceContainer.refresh")}
    </button>
  </>;
}

export default function SourceContainer({ location, search, filter, lens, onSearchChange, onFilterChange, onLensChange, onClearFilters, onNavigate, onAdd, onOpenDocument }: Props) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [selectedFile, setSelectedFile] = useState<SourceDirEntry | null>(null);
  const sourcesQuery = useQuery({ queryKey: ["registeredSources"], queryFn: listRegisteredSources });
  const filesQuery = useQuery({ queryKey: ["indexedFiles"], queryFn: () => listIndexedFiles(), refetchInterval: 5000 });
  const source = sourcesQuery.data?.find((item) => item.id === location.sourceId);
  const dirQuery = useQuery({
    queryKey: ["sourceDir", location.path],
    queryFn: () => readSourceDir(location.path),
    enabled: !!source,
  });
  const infoState = useDetailInfoOpen(`source-container:${location.sourceId}`);
  const infoToggleRef = useRef<HTMLButtonElement>(null);
  const refreshMutation = useMutation({
    mutationFn: () => syncRegisteredSource(location.sourceId),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["registeredSources"] }),
        queryClient.invalidateQueries({ queryKey: ["indexedFiles"] }),
        queryClient.invalidateQueries({ queryKey: ["sourceDir"] }),
      ]);
    },
    onError: (error) => {
      console.warn("[sources] refresh failed", error);
      toast(t("sourceContainer.refreshFailed"));
    },
  });

  const indexedByPath = useMemo(() => {
    const result = new Map<string, IndexedFileInfo>();
    for (const file of filesQuery.data ?? []) {
      const id = (file.source === "obsidian" || isNoteApp(file.source_agent))
        ? file.source_id.replace(/::\d+$/, "") : file.source_id;
      const prior = result.get(id);
      result.set(id, { ...file, source_id: id,
        chunk_count: file.chunk_count + (prior?.chunk_count ?? 0),
        last_modified: Math.max(file.last_modified, prior?.last_modified ?? 0) });
    }
    return result;
  }, [filesQuery.data]);
  const entries = useMemo(() => [...(dirQuery.data ?? [])].sort((a, b) => {
    if (a.isDirectory !== b.isDirectory) return a.isDirectory ? -1 : 1;
    return a.name.localeCompare(b.name);
  }), [dirQuery.data]);
  const needle = search.trim().toLocaleLowerCase();
  const items: SourceCollectionItem[] = entries.filter((entry) => {
    const file = !entry.isDirectory ? indexedByPath.get(`${location.sourceId}::${childPath(location.path, entry.name)}`) : undefined;
    const isLink = file?.source === "webpage";
    const app = isLink && file ? getSourceAppFromUrl(file.url ?? file.source_id) : null;
    return (filter === "all" || (filter === "folders" ? entry.isDirectory : filter === "links" ? isLink && !app
      : filter === "files" ? !entry.isDirectory && !isLink : app === filter)) &&
      entry.name.toLocaleLowerCase().includes(needle);
  }).map((entry) => {
    const path = childPath(location.path, entry.name);
    const indexed = indexedByPath.get(`${location.sourceId}::${path}`);
    const presentation = getSourcePresentation(indexed ?? { source: "file", source_id: path, title: entry.name }, { document: t("sourceLibrary.document"), webExcerpt: t("sourceLibrary.webExcerpt") });
    const kind = entry.isDirectory ? "folder" : presentation.iconKind;
    return {
      id: path,
      title: entry.name,
      metadata: entry.isDirectory ? t("sourceLibrary.folder") : presentation.format,
      kind,
      onSelect: () => {
        if (entry.isDirectory) {
          setSelectedFile(null);
          onNavigate({ sourceId: location.sourceId, path });
        } else if (supportedText.has(extension(entry.name))) {
          setSelectedFile(entry);
        } else if (indexed) {
          onOpenDocument(indexed);
        } else {
          void openFile(path).catch((error) => {
            console.warn("[sources] open file failed", error);
            toast(t("sourceAccess.openFileFailed"));
          });
        }
      },
    };
  });

  const pathParts = normalized(location.path).split("/").filter(Boolean);
  const rootParts = normalized(source?.path ?? "").split("/").filter(Boolean);
  const crumbs = pathParts.slice(rootParts.length);
  const rootName = basename(source?.path ?? "");
  const selectedPath = selectedFile ? childPath(location.path, selectedFile.name) : "";
  const selectedIndexed = selectedFile ? indexedByPath.get(`${location.sourceId}::${selectedPath}`) : undefined;
  const localFile: IndexedFileInfo | null = selectedFile ? (selectedIndexed ? { ...selectedIndexed, title: selectedFile.name } : {
    source: "file", source_agent: source?.source_type ?? "folder", source_id: selectedPath,
    title: selectedFile.name, chunk_count: 0, last_modified: 0,
  }) : null;
  const textQuery = useQuery({
    queryKey: ["fileText", selectedPath],
    queryFn: () => readSourceText(selectedPath),
    enabled: !!selectedFile,
    retry: false,
  });

  if (!source && sourcesQuery.isPending) return <section className="source-container-workspace"><p role="status">{t("sourceContainer.loading")}</p></section>;
  if (!source && sourcesQuery.isError) return <section className="source-container-workspace"><div role="alert">{t("sourceLibrary.folderError")} <button type="button" onClick={() => void sourcesQuery.refetch()}>{t("sourceLibrary.retryFolders")}</button></div></section>;
  if (!source) return <section className="source-container-workspace"><div role="alert">{t("sourceLibrary.folderError")} <button type="button" onClick={() => onNavigate(null)}>{t("sourceContainer.root")}</button></div></section>;

  if (selectedFile && localFile) return <div className="source-container-workspace source-container-reader">
    <SourceDocumentLayout file={localFile} breadcrumbs={<SourceCollectionBreadcrumbs>
      <button className="source-container-back" type="button" onClick={() => setSelectedFile(null)}><ArrowLeft size={14} aria-hidden="true" />{t("sourceContainer.backToFolder")}</button>
    </SourceCollectionBreadcrumbs>} content={<div className="source-container-file-content">
      {textQuery.isPending && <p role="status">{t("sourceAccess.previewLoading")}</p>}
      {textQuery.isError && <div role="alert"><p>{t("sourceContainer.fileReadError")}</p><button type="button" className="page-editor-action" onClick={() => void textQuery.refetch()}>{t("sourceContainer.retryFile")}</button></div>}
      {!textQuery.isPending && !textQuery.isError && textQuery.data?.trim()
        ? <SourceDocumentContent content={textQuery.data} title={documentTitle(localFile.title)} />
        : !textQuery.isPending && !textQuery.isError && <p>{t("sourceAccess.noPreview")}</p>}
    </div>} infoContent={<><section>
      <h3 className="note-info-section-heading">{t("sourceAccess.index")}</h3>
      <dl className="source-info-metadata">
        <div><dt>{t("sourceAccess.status")}</dt><dd>{selectedIndexed ? t("sourceAccess.inLibrary") : t("sourceAccess.indexing")}</dd></div>
        {selectedIndexed && <div><dt>{t("sourceAccess.indexedAs")}</dt><dd>{t(selectedIndexed.chunk_count === 1 ? "sourceAccess.onePassage" : "sourceAccess.passages", { count: selectedIndexed.chunk_count })}</dd></div>}
        <div><dt>{t("sourceAccess.localFile")}</dt><dd className="source-container-path">{selectedPath}</dd></div>
      </dl>
    </section><SourceContainerInfo source={source} heading={t("sourceContainer.connection")} refreshing={refreshMutation.isPending} onRefresh={() => refreshMutation.mutate()} /></>} />
  </div>;

  return <section className={`source-container-workspace document-context-host${infoState.open ? " document-context-open" : ""}`}>
    <main className="source-container-main">
      <CollectionFrame className="source-container-collection">
        <SourceCollectionBreadcrumbs>
          <button type="button" onClick={() => onNavigate(null)}>{t("sourceContainer.root")}</button><CaretRight size={14} aria-hidden="true" />
          {isNoteApp(source.source_type) && <SourceTypeIcon kind={source.source_type} size={16} />}
          <button type="button" onClick={() => onNavigate({ sourceId: source.id, path: source.path })} aria-current={location.path === source.path ? "page" : undefined}>{rootName}</button>
          {crumbs.map((part, index) => <span className="source-container-crumb" key={`${index}:${part}`}>
            <CaretRight size={14} aria-hidden="true" />
            {index === crumbs.length - 1 ? <span aria-current="page">{part}</span> : <button type="button" onClick={() => onNavigate({ sourceId: source.id, path: `${normalized(source.path)}/${crumbs.slice(0, index + 1).join("/")}` })}>{part}</button>}
          </span>)}
        </SourceCollectionBreadcrumbs>
        <SourceCollectionToolbar search={search} filter={filter} lens={lens} onSearchChange={onSearchChange} onFilterChange={onFilterChange}
          onLensChange={onLensChange} onAdd={onAdd} trailing={<InfoToggle ref={infoToggleRef} open={infoState.open} onClick={infoState.toggle} />} />
        <div className="source-container-notices">
          {dirQuery.isPending && <p role="status">{t("sourceContainer.loading")}</p>}
          {dirQuery.isError && <div role="alert"><span>{t("sourceContainer.folderReadError")}</span><button type="button" onClick={() => void dirQuery.refetch()}>{t("sourceContainer.retryFolder")}</button></div>}
          {filesQuery.isError && <div role="alert"><span>{t("sourceLibrary.documentError")}</span><button type="button" onClick={() => void filesQuery.refetch()}>{t("sourceLibrary.retryDocuments")}</button></div>}
        </div>
        {!dirQuery.isPending && !dirQuery.isError && items.length > 0 && <SourceCollection items={items} lens={lens} label={t("sourceContainer.containerList")} />}
        {!dirQuery.isPending && !dirQuery.isError && items.length === 0 && <div className="source-container-empty">
          <SourceTypeIcon kind="folder" size={28} /><p>{t(entries.length > 0 ? "sourceLibrary.noMatches" : "sourceContainer.empty")}</p>
          {entries.length > 0 && <button className="collection-control" type="button" onClick={onClearFilters}>{t("sourceLibrary.clearFilters")}</button>}
        </div>}
      </CollectionFrame>
    </main>
    <PageInfoDrawer docked inspector focusOnOpen={false} returnFocusRef={infoToggleRef}
      open={infoState.open} onClose={() => infoState.setOpen(false)} title={t("knowledgeContext.memoryPanel")} closeLabel={t("common.close")}>
      <DetailInfoContent className="source-container-info">
        <SourceContainerInfo source={source} refreshing={refreshMutation.isPending} onRefresh={() => refreshMutation.mutate()} />
      </DetailInfoContent>
    </PageInfoDrawer>
  </section>;
}
