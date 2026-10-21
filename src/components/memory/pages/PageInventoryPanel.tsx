// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { CaretRight, Folder, FolderPlus, MagnifyingGlass, Plus, Stack } from "@phosphor-icons/react";
import { PageIcon } from "../assets/PageIcon";
import { createPortal } from "react-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { deletePage, discardPageDraft, getPage, knowledgeFolderCreate, type Page, type KnowledgeFolder } from "../../../lib/tauri";
import { listAllActivePages, listAllDraftPages, listAllActivePagesExplicitBrowse, listAllDraftPagesExplicitBrowse, EXPLICIT_BROWSE_QUERY_POLICY } from "./listAllPages";
import { pageSpaceContext } from "./pagePresentation";
import { collectPageInventory, filterPageInventory, folderScope, inventoryFolderPath, pageFolderPath, pageMatchesInventoryScope, type WikiInventoryScope } from "./pageInventory";
import { KNOWLEDGE_FOLDERS_QUERY_KEY, useKnowledgeFolders } from "./useKnowledgeFolders";
import { useWikiFolderCreation } from "./WikiFolderCreationContext";
import "./pageInventory.css";

type PageInventoryPanelProps = {
  readonly currentPageId?: string | null;
  readonly currentFolderPath?: string | null;
  readonly fileTree?: boolean;
  readonly inventoryScope?: WikiInventoryScope;
  readonly browsing?: boolean;
  readonly filter?: string;
  readonly onFilterChange?: (filter: string) => void;
  readonly onBrowse?: (scope: WikiInventoryScope) => void;
  readonly onCreatePage?: (folderPath?: string) => void;
  readonly onOpenDraft?: (draftId: string, space: string | null) => void;
  readonly onOpenPage?: (page: Page) => void;
  /** File-tree mode: called after a note (published page or draft) was deleted from the directory, so the host can close its tab. */
  readonly onDeletePage?: (pageId: string) => void;
  /** File-tree mode: the number of notes the tree lists, or null until it has loaded. */
  readonly onNoteCountChange?: (count: number | null) => void;
};

type FolderContextMenuState = {
  readonly path: string;
  /** Set when the menu was opened on a note row; absent for folders and empty space. */
  readonly noteId?: string;
  readonly x: number;
  readonly y: number;
  readonly invoker: HTMLElement;
};

export function PageInventoryPanel({ currentPageId = null, currentFolderPath, fileTree = false, inventoryScope = "all", browsing = false, filter: controlledFilter, onFilterChange, onBrowse, onCreatePage, onOpenDraft, onOpenPage, onDeletePage, onNoteCountChange }: PageInventoryPanelProps) {
  const { i18n, t } = useTranslation();
  const id = useId();
  const queryClient = useQueryClient();
  const [localFilter, setLocalFilter] = useState("");
  const filter = controlledFilter ?? localFilter;
  const setFilter = onFilterChange ?? setLocalFilter;
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set([""]));
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [createPending, setCreatePending] = useState(false);
  const [createError, setCreateError] = useState(false);
  const [folderFormParentPath, setFolderFormParentPath] = useState("");
  const [contextMenu, setContextMenu] = useState<FolderContextMenuState | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);
  const contextMenuItemRef = useRef<HTMLButtonElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const deletingRef = useRef(false);
  const folderFormInvokerRef = useRef<HTMLElement | null>(null);
  const folderCreation = useWikiFolderCreation();
  const explicitBrowse = browsing || fileTree;
  const active = useQuery({ queryKey: explicitBrowse ? ["pages", "active"] : ["pages", "inventory", "passive", "active"], queryFn: explicitBrowse ? listAllActivePagesExplicitBrowse : listAllActivePages, ...(explicitBrowse ? EXPLICIT_BROWSE_QUERY_POLICY : { staleTime: 30_000 }) });
  const drafts = useQuery({ queryKey: explicitBrowse ? ["pages", "draft"] : ["pages", "inventory", "passive", "draft"], queryFn: explicitBrowse ? listAllDraftPagesExplicitBrowse : listAllDraftPages, ...(explicitBrowse ? EXPLICIT_BROWSE_QUERY_POLICY : { staleTime: 30_000 }) });
  const folders = useKnowledgeFolders();
  const pages = useMemo(() => collectPageInventory(active.data ?? [], drafts.data ?? []), [active.data, drafts.data]);
  const noteCount = fileTree && active.isSuccess && drafts.isSuccess ? pages.length : null;
  useEffect(() => { onNoteCountChange?.(noteCount); }, [noteCount, onNoteCountChange]);
  const selectedPage = pages.find(page => page.id === currentPageId);
  const selectedPath = selectedPage ? pageFolderPath(selectedPage) : currentFolderPath !== undefined ? currentFolderPath : inventoryFolderPath(inventoryScope);
  const [creationPath, setCreationPath] = useState<string | null>(selectedPath);
  // Follow the selection during render, not in an effect. The workspace feeds
  // the reported path back in as currentFolderPath, and an effect's one-render
  // lag let two values chase each other forever (React's update-depth error).
  const [creationBasis, setCreationBasis] = useState({ selectedPath, currentPageId });
  if (creationBasis.selectedPath !== selectedPath || creationBasis.currentPageId !== currentPageId) {
    setCreationBasis({ selectedPath, currentPageId });
    setCreationPath(selectedPath);
  }
  useEffect(() => { folderCreation?.reportCreationFolderPath(creationPath); }, [creationPath, folderCreation?.reportCreationFolderPath]);
  const toggleFolder = (path: string) => {
    setCreationPath(path);
    setExpanded(previous => { const next = new Set(previous); if (next.has(path)) next.delete(path); else next.add(path); return next; });
  };
  useEffect(() => {
    if (selectedPath === null) return;
    setExpanded(previous => {
      const next = new Set(previous); next.add("");
      const parts = selectedPath.split("/");
      for (let index = 1; index <= parts.length; index++) next.add(parts.slice(0, index).join("/"));
      return next;
    });
  }, [selectedPath]);
  const hasFilter = !fileTree && !!filter.trim();
  const canCreate = !!folders.data && !folders.data.truncated;
  const folderUnavailable = folders.isError || !!folders.data?.truncated;
  const createStateRef = useRef({ canCreate, createPending, creating });
  const requestedCreatePath = (fileTree ? creationPath : selectedPath) ?? "";
  const defaultCreatePath = requestedCreatePath === "" || folders.data?.folders.some(folder => folder.path === requestedCreatePath)
    ? requestedCreatePath
    : "";
  const defaultCreatePathRef = useRef(defaultCreatePath);
  createStateRef.current = { canCreate, createPending, creating };
  defaultCreatePathRef.current = defaultCreatePath;
  const openCreateFolderFormAt = useCallback((path: string, invoker: HTMLElement | null) => {
    const state = createStateRef.current;
    if (!state.canCreate || state.createPending || state.creating) return;
    setFolderFormParentPath(path);
    folderFormInvokerRef.current = invoker;
    setCreating(true);
    setCreateError(false);
  }, []);
  const openCreateFolderForm = useCallback(() => {
    const activeElement = document.activeElement;
    openCreateFolderFormAt(defaultCreatePathRef.current, activeElement instanceof HTMLElement ? activeElement : null);
  }, [openCreateFolderFormAt]);
  useEffect(() => {
    if (!folderCreation) return;
    return folderCreation.registerCreateFolderForm(openCreateFolderForm);
  }, [folderCreation?.registerCreateFolderForm, openCreateFolderForm]);
  useEffect(() => {
    folderCreation?.reportCanCreateFolder(canCreate);
  }, [canCreate, folderCreation?.reportCanCreateFolder]);
  const closeFolderForm = (restoreFocus: boolean) => {
    setCreating(false);
    setCreateError(false);
    if (restoreFocus) requestAnimationFrame(() => folderFormInvokerRef.current?.focus());
  };
  const openContextMenu = (path: string, invoker: HTMLElement, x: number, y: number, noteId?: string) => {
    setContextMenu({ path, invoker, x, y, ...(noteId ? { noteId } : {}) });
  };
  const contextTarget = (target: EventTarget | null, fallback: HTMLElement) => {
    const element = target instanceof Element ? target : null;
    if (!element) return { path: "", invoker: fallback };
    if (element.closest("form, input, textarea, select, [contenteditable='true']")) return null;
    const note = element.closest<HTMLElement>("[data-page-id]");
    if (note?.dataset.pageId) {
      return { path: note.dataset.folderPath ?? "", noteId: note.dataset.pageId, invoker: note.querySelector<HTMLElement>(".notes-inventory-page") ?? fallback };
    }
    const folder = element.closest<HTMLElement>("[data-folder-path]");
    const path = folder?.dataset.folderPath ?? "";
    const invoker = folder?.querySelector<HTMLElement>(".notes-inventory-scope") ?? (element.closest<HTMLElement>("button") ?? fallback);
    return { path, invoker };
  };
  const handleContextMenu = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = contextTarget(event.target, event.currentTarget);
    if (!target) return;
    event.preventDefault();
    openContextMenu(target.path, target.invoker, event.clientX, event.clientY, target.noteId);
  };
  const deleteNote = async (noteId: string) => {
    const note = pages.find(candidate => candidate.id === noteId);
    if (!note || deletingRef.current) return;
    if (!confirm(t("pageDetail.deleteConfirm"))) return;
    deletingRef.current = true;
    try {
      if (note.status === "draft") {
        // delete_page rejects drafts; a draft is discarded at its latest version.
        // An open editor can autosave between the read and the discard, so a
        // version conflict reads the version again once.
        for (let attempt = 0; ; attempt++) {
          const latest = await getPage(noteId);
          if (!latest) break;
          try { await discardPageDraft({ id: noteId, expectedVersion: latest.version }); break; }
          catch (cause) {
            const code = cause && typeof cause === "object" && "code" in cause ? (cause as { code?: unknown }).code : null;
            if (code === "page_draft_not_found") break;
            if (code !== "draft_version_conflict" || attempt > 0) throw cause;
          }
        }
      } else await deletePage(noteId);
      void queryClient.invalidateQueries({ queryKey: ["page", noteId], refetchType: "none" });
      void queryClient.invalidateQueries({ queryKey: ["pages"] });
      void queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
      void queryClient.invalidateQueries({ queryKey: ["page-links", noteId], refetchType: "none" });
      void queryClient.invalidateQueries({ queryKey: ["page-revisions", noteId], refetchType: "none" });
      void queryClient.invalidateQueries({ queryKey: ["page-sources", noteId], refetchType: "none" });
      onDeletePage?.(noteId);
      requestAnimationFrame(() => {
        const focused = document.activeElement;
        if (!focused || focused === document.body) scrollRef.current?.focus();
      });
    } catch {
      toast(t("pageDetail.deleteError"));
    } finally { deletingRef.current = false; }
  };
  const handleContextMenuKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if ((event.key === "Delete" || event.key === "Backspace") && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey) {
      const row = event.target instanceof HTMLElement && event.target.matches(".notes-inventory-page") ? event.target.closest<HTMLElement>("[data-page-id]") : null;
      if (row?.dataset.pageId) { event.preventDefault(); void deleteNote(row.dataset.pageId); }
      return;
    }
    if (event.key !== "ContextMenu" && !(event.key === "F10" && event.shiftKey)) return;
    const target = contextTarget(event.target, event.currentTarget);
    if (!target) return;
    event.preventDefault();
    const rect = target.invoker.getBoundingClientRect();
    openContextMenu(target.path, target.invoker, rect.left, rect.bottom, target.noteId);
  };
  const contextNote = contextMenu?.noteId ? pages.find(candidate => candidate.id === contextMenu.noteId) : undefined;
  const openContextNote = () => {
    if (!contextNote) return;
    const draft = contextNote.status === "draft";
    if (draft ? !onOpenDraft : !onOpenPage) return;
    setContextMenu(null);
    if (draft) onOpenDraft?.(contextNote.id, pageSpaceContext(contextNote) ?? null);
    else onOpenPage?.(contextNote);
  };
  const dismissContextMenu = (restoreFocus: boolean) => {
    if (restoreFocus) contextMenu?.invoker.focus();
    setContextMenu(null);
  };
  useLayoutEffect(() => {
    if (!contextMenu) return;
    const menu = contextMenuRef.current;
    if (!menu) return;
    const bounds = menu.getBoundingClientRect();
    const x = Math.max(8, Math.min(contextMenu.x, window.innerWidth - bounds.width - 8));
    const y = Math.max(8, Math.min(contextMenu.y, window.innerHeight - bounds.height - 8));
    if (x !== contextMenu.x || y !== contextMenu.y) setContextMenu(current => current ? { ...current, x, y } : current);
    contextMenuItemRef.current?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!contextMenuRef.current?.contains(event.target as Node)) dismissContextMenu(false);
    };
    const onViewportChange = () => dismissContextMenu(false);
    document.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("scroll", onViewportChange, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("resize", onViewportChange);
      window.removeEventListener("scroll", onViewportChange, true);
    };
  }, [contextMenu]);
  const selectFolder = (path: string) => {
    setFilter("");
    if (fileTree) toggleFolder(path);
    else onBrowse?.(folderScope(path));
  };
  const renderPage = (page: Page) => {
    const draft = page.status === "draft";
    const title = draft && !page.title.trim() ? t("pages.overview.untitledDraft") : page.title;
    return <li key={page.id} data-folder-path={pageFolderPath(page) ?? ""} data-page-id={page.id} data-page-status={page.status}><button type="button" className="notes-page-button notes-inventory-page" aria-label={t("pages.overview.openPage", { title })} aria-current={currentPageId === page.id ? "page" : undefined} data-active={currentPageId === page.id ? "true" : undefined} title={title} disabled={draft ? !onOpenDraft : !onOpenPage} onClick={() => draft ? onOpenDraft?.(page.id, pageSpaceContext(page) ?? null) : onOpenPage?.(page)}><PageIcon size={16}/><span className="notes-page-title">{title}</span></button></li>;
  };
  const renderFolder = (folder: KnowledgeFolder, depth: number): React.ReactNode => {
    const children = (folders.data?.folders ?? []).filter(item => item.path !== folder.path && item.parent_path === folder.path);
    const noteChildren = (fileTree || !browsing) ? pages.filter(page => pageMatchesInventoryScope(page, folderScope(folder.path))) : [];
    const canExpand = children.length > 0 || noteChildren.length > 0;
    const open = canExpand && expanded.has(folder.path);
    const listId = `${id}-${folder.path}`;
    const folderActive = fileTree ? creationPath === folder.path && !currentPageId : selectedPath === folder.path;
    return <li key={folder.path} data-folder-path={folder.path}>
      <div className="notes-inventory-collection-row" data-active={folderActive ? "true" : undefined} style={{ marginLeft: depth * 14 }}>
        {canExpand ? <button type="button" className="notes-inventory-disclosure" aria-label={t(open ? "pages.inventory.collapse" : "pages.inventory.expand", { name: folder.name })} aria-expanded={open} aria-controls={listId} onClick={() => toggleFolder(folder.path)}><CaretRight aria-hidden="true" size={12}/></button> : <span className="notes-inventory-disclosure-spacer" aria-hidden="true"/>}
        <button type="button" className="notes-inventory-scope" data-active={folderActive ? "true" : undefined} aria-current={!fileTree && selectedPath === folder.path ? "page" : undefined} onClick={() => selectFolder(folder.path)} disabled={!fileTree && !onBrowse}><Folder aria-hidden="true" size={16}/><span>{folder.name}</span></button>
        {fileTree && <button type="button" className="notes-inventory-folder-action" aria-label={t("pages.folders.newFolderIn", { name: folder.name })} title={t("pages.folders.newFolderIn", { name: folder.name })} disabled={!canCreate || createPending || creating} onClick={event => openCreateFolderFormAt(folder.path, event.currentTarget)}><FolderPlus aria-hidden="true" size={14}/></button>}
      </div>
      {fileTree && creating && folderFormParentPath === folder.path && folderCreateForm}
      {open && <ul id={listId} className="notes-inventory-collections">{children.map(child => renderFolder(child, depth + 1))}{noteChildren.length > 0 && <li><ul className="notes-page-list notes-inventory-pages" style={{ paddingLeft: depth * 14 }}>{noteChildren.map(renderPage)}</ul></li>}</ul>}
    </li>;
  };
  const create = async (event: React.FormEvent) => {
    event.preventDefault(); if (!name.trim() || createPending || !canCreate) return;
    setCreatePending(true); setCreateError(false);
    try {
      const parentPath = folderFormParentPath;
      const result = await knowledgeFolderCreate(parentPath, name.trim());
      queryClient.setQueryData(KNOWLEDGE_FOLDERS_QUERY_KEY, { folders: [...(folders.data?.folders ?? []), { path: result.path, parent_path: parentPath, name: name.trim() }], truncated: false });
      setCreating(false); setName("");
      if (fileTree) {
        setCreationPath(result.path);
        setExpanded(previous => new Set([...previous, parentPath, result.path]));
      } else onBrowse?.(folderScope(result.path));
      void queryClient.invalidateQueries({ queryKey: KNOWLEDGE_FOLDERS_QUERY_KEY });
    } catch { setCreateError(true); } finally { setCreatePending(false); }
  };
  const folderCreateForm = creating && <form className="notes-folder-create" onSubmit={create} onKeyDown={event => { if (event.key === "Escape" && !createPending) { event.preventDefault(); event.stopPropagation(); closeFolderForm(true); } }}>
    <label htmlFor={`${id}-name`}>{t("pages.folders.folderName")}</label>
    <input id={`${id}-name`} autoFocus value={name} disabled={createPending} maxLength={100} onChange={event => setName(event.target.value)} />
    <span className="notes-folder-parent">{t("pages.folders.createIn", { name: folderFormParentPath || t("pages.folders.root") })}</span>
    <div>
      <button type="submit" disabled={createPending || !name.trim()}>{t("pages.folders.create")}</button>
      <button type="button" onClick={() => closeFolderForm(true)} disabled={createPending}>{t("pages.folders.cancel")}</button>
    </div>
    {createError && <p role="alert">{t("pages.folders.createError")}</p>}
  </form>;
  return <section aria-label={fileTree ? undefined : t("sidebar.notes")} className={`notes-list-panel notes-inventory-panel${fileTree ? " notes-inventory-panel--file-tree" : ""}`}>
    {fileTree ? <h2 className="sr-only">{t("pages.folders.title")}</h2> : <div className="notes-list-header"><h2>{t("sidebar.notes")}</h2><div className="flex items-center gap-1">
      {onCreatePage && <button type="button" className="notes-list-create" aria-label={t("sidebar.newNote")} title={t("sidebar.newNote")} onClick={() => { if (selectedPath === null) onCreatePage(); else onCreatePage(selectedPath); }}><Plus aria-hidden="true" size={16}/></button>}
      <button type="button" className="notes-list-create" aria-label={t("pages.folders.newFolder")} title={t("pages.folders.newFolder")} disabled={!canCreate || createPending} onClick={event => { if (creating) closeFolderForm(false); else openCreateFolderFormAt(selectedPath ?? "", event.currentTarget); }}><FolderPlus aria-hidden="true" size={16}/></button>
    </div></div>}
    {!fileTree && folderCreateForm}
    {!fileTree && <label className="notes-list-filter"><span className="sr-only">{t("pages.folders.searchAll")}</span><MagnifyingGlass aria-hidden="true" size={14}/><input aria-label={t("pages.folders.searchAll")} placeholder={t("pages.folders.searchAll")} onChange={event => setFilter(event.target.value)} type="search" value={filter}/></label>}
    <div className="notes-list-scroll" ref={scrollRef} tabIndex={fileTree ? 0 : undefined} role={fileTree ? "group" : undefined} aria-label={fileTree ? t("pages.folders.title") : undefined} onContextMenu={fileTree ? handleContextMenu : undefined} onKeyDown={fileTree ? handleContextMenuKey : undefined}>
      {(active.isPending || drafts.isPending) ? <p className="notes-list-state" role="status">{t("pages.overview.loading")}</p> : (active.isError || drafts.isError) ? <div className="notes-list-state" role="alert"><p>{t("pages.overview.error")}</p><button type="button" onClick={() => { void active.refetch(); void drafts.refetch(); }}>{t("pageDetail.retry")}</button></div> : <>
        {!fileTree && <button type="button" className="notes-inventory-scope notes-inventory-all" aria-label={t("pages.inventory.all")} data-active={selectedPath === null ? "true" : undefined} aria-current={selectedPath === null ? "page" : undefined} onClick={() => { setFilter(""); onBrowse?.("all"); }} disabled={!onBrowse}><Stack aria-hidden="true" size={16}/><span>{t("pages.inventory.all")}</span></button>}
        {hasFilter ? <><p className="notes-folder-parent">{t("pages.folders.searchAll")}</p><ul className="notes-page-list">{filterPageInventory(pages, filter, i18n.language).map(renderPage)}</ul>{!filterPageInventory(pages, filter, i18n.language).length && <p className="notes-list-state">{t("pages.overview.noMatches")}</p>}</> : <>
          {fileTree && folders.data && !folders.data.truncated && folders.data.folders.length === 0 && <p className="notes-list-state wiki-empty-directory">{t("pages.folders.emptyDirectory")}</p>}
          {folders.data && !folders.data.truncated && (fileTree ? <ul className="notes-inventory-collections">
            {folders.data.folders.filter(folder => folder.parent_path === "").map(folder => renderFolder(folder, 0))}
          </ul> : <ul className="notes-inventory-collections">{renderFolder({ path: "", parent_path: "", name: t("pages.folders.root") }, 0)}</ul>)}
          {fileTree && <ul className="notes-page-list notes-inventory-root-pages">{pages.filter(page => {
            const path = pageFolderPath(page);
            return folderUnavailable || !path || !folders.data?.folders.some(folder => folder.path === path);
          }).map(renderPage)}</ul>}
          {!fileTree && !browsing && selectedPath === null && <ul className="notes-page-list">{pages.filter(page => pageFolderPath(page) === null).map(renderPage)}</ul>}
          {fileTree && folderFormParentPath === "" && folderCreateForm}
        </>}
        {folderUnavailable && <p className="notes-list-state" role="status">{t("pages.folders.unavailable")}</p>}
      </>}
    </div>
    {contextMenu && createPortal(<div className="mem-popover-surface notes-folder-context-menu" role="menu" aria-label={t("pages.folders.title")} ref={contextMenuRef} style={{ left: contextMenu.x, top: contextMenu.y }} onKeyDown={event => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); dismissContextMenu(true); }
      else if (event.key === "Tab") dismissContextMenu(false);
      else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("[role='menuitem']"));
        const index = items.indexOf(document.activeElement as HTMLElement);
        if (items.length > 1) { event.preventDefault(); items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus(); }
      }
    }}>
      {contextMenu.noteId ? <>
        <button type="button" role="menuitem" ref={contextMenuItemRef} aria-disabled={!contextNote || (contextNote.status === "draft" ? !onOpenDraft : !onOpenPage)} onClick={openContextNote}>{t("pages.folders.openNote")}</button>
        <button type="button" role="menuitem" aria-disabled={!contextNote} onClick={() => { if (!contextNote) return; const { id: noteId } = contextNote; setContextMenu(null); void deleteNote(noteId); }}>{t("pages.folders.deleteNote")}</button>
      </> : <button type="button" role="menuitem" ref={contextMenuItemRef} aria-disabled={!canCreate || createPending || creating} onClick={() => { if (!canCreate || createPending || creating) return; const { path, invoker } = contextMenu; setContextMenu(null); openCreateFolderFormAt(path, invoker); }}>{t("pages.folders.newFolderIn", { name: contextMenu.path || t("pages.folders.root") })}</button>}
    </div>, document.body)}
  </section>;
}
