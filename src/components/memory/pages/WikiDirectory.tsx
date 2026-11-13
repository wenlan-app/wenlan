// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { CaretLeft, FolderPlus, NotePencil, Plus } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import type { Page } from "../../../lib/tauri";
import { ArchivedNotesPanel } from "./ArchivedNotesPanel";
import { PageInventoryPanel } from "./PageInventoryPanel";
import { WikiFolderCreationContext } from "./WikiFolderCreationContext";
import type { WikiInventoryScope } from "./pageInventory";
import { SpaceFoldersPrompt } from "./SpaceFoldersPrompt";

/** The id of the one directory the shell renders (see Main). */
export const WIKI_DIRECTORY_ID = "wiki-directory";

/** The directory shows the folder tree. Archived pages is a short detour,
 *  opened from the home, never a mode you pick (Qi-Xuan 2026-11-09 19:14 PT:
 *  no browse-mode menu; archived pages stay out of the tree, as in Obsidian). */
export type WikiInventoryMode = "folders" | "archived";

export interface WikiDirectoryProps {
  readonly id?: string;
  /** Where the directory sits: its own column in the Wiki workspace, or a
   *  section of the app's one left sidebar, below the destinations. */
  readonly placement?: "workspace" | "sidebar";
  /** Leads the header row in the sidebar: the Wiki destination itself. */
  readonly heading?: ReactNode;
  readonly inventoryScope?: WikiInventoryScope;
  readonly currentPageId?: string | null;
  readonly currentFolderPath?: string | null;
  readonly inventoryMode?: WikiInventoryMode;
  readonly onInventoryModeChange?: (mode: WikiInventoryMode) => void;
  readonly browsing?: boolean;
  readonly filter?: string;
  readonly onFilterChange?: (filter: string) => void;
  readonly onBrowse: (scope: WikiInventoryScope) => void;
  readonly onCreatePage: (folderPath?: string) => void;
  readonly onCreationFolderPathChange?: (path: string | null) => void;
  readonly onOpenDraft: (draftId: string, space: string | null) => void;
  readonly onOpenPage: (page: Page) => void;
  readonly onDeletePage?: (pageId: string) => void;
  readonly onArchiveOpenPage?: (pageId: string) => Promise<boolean>;
  readonly onRestorePage?: (pageId: string) => void;
  /** Runs before every navigation the directory starts (a narrow drawer closes). */
  readonly onNavigate?: () => void;
  /** Asks the narrow drawer to open, e.g. before a new folder form appears. */
  readonly onRevealRequest?: () => void;
  /** Rendered inside the folder-creation context, after the list (tests). */
  readonly children?: ReactNode;
}

/** Every page of the Wiki, by folder; or, for a moment, the archived pages. */
export function WikiDirectory({
  id,
  placement = "workspace",
  heading,
  inventoryScope = "all",
  currentPageId = null,
  currentFolderPath,
  inventoryMode: controlledInventoryMode,
  onInventoryModeChange,
  browsing = false,
  filter,
  onFilterChange,
  onBrowse,
  onCreatePage,
  onCreationFolderPathChange,
  onOpenDraft,
  onOpenPage,
  onDeletePage,
  onArchiveOpenPage,
  onRestorePage,
  onNavigate,
  onRevealRequest,
  children,
}: WikiDirectoryProps) {
  const { t } = useTranslation();
  const [localInventoryMode, setLocalInventoryMode] = useState<WikiInventoryMode>("folders");
  const inventoryMode = controlledInventoryMode ?? localInventoryMode;
  const [creationFolderPath, reportCreationFolderPath] = useState<string | null>(null);
  const exampleWiki = useExampleWikiActive();
  useEffect(() => {
    onCreationFolderPathChange?.(inventoryMode === "folders" ? creationFolderPath : null);
  }, [creationFolderPath, inventoryMode, onCreationFolderPathChange]);
  const [canCreateFolder, setCanCreateFolder] = useState(false);
  const [noteCount, setNoteCount] = useState<number | null>(null);
  const createFormRef = useRef<(() => void) | null>(null);
  const reportCanCreateFolder = useCallback((available: boolean) => {
    setCanCreateFolder(current => current === available ? current : available);
  }, []);
  const registerCreateFolderForm = useCallback((openForm: () => void) => {
    createFormRef.current = openForm;
    return () => {
      if (createFormRef.current !== openForm) return;
      createFormRef.current = null;
      reportCanCreateFolder(false);
    };
  }, [reportCanCreateFolder]);
  const requestCreateFolder = useCallback(() => {
    if (inventoryMode !== "folders" || !canCreateFolder || !createFormRef.current) return false;
    onRevealRequest?.();
    createFormRef.current();
    return true;
  }, [canCreateFolder, inventoryMode, onRevealRequest]);
  const folderCreationContext = useMemo(() => ({
    creationFolderPath: inventoryMode === "folders" ? creationFolderPath : null,
    reportCreationFolderPath,
    canCreateFolder: inventoryMode === "folders" && canCreateFolder,
    reportCanCreateFolder,
    registerCreateFolderForm,
    requestCreateFolder,
  }), [creationFolderPath, inventoryMode, canCreateFolder, reportCanCreateFolder, registerCreateFolderForm, requestCreateFolder]);
  const changeInventoryMode = (mode: WikiInventoryMode) => {
    setLocalInventoryMode(mode);
    onInventoryModeChange?.(mode);
  };
  const closeForNavigation = <Arguments extends readonly unknown[]>(callback: (...args: Arguments) => void) => (...args: Arguments) => {
    onNavigate?.();
    callback(...args);
  };
  const inSidebar = placement === "sidebar";

  return (
    <WikiFolderCreationContext.Provider value={folderCreationContext}>
    <aside id={id} className={`wiki-workspace-directory${inSidebar ? " wiki-sidebar-directory" : ""}`}>
      <div className="wiki-workspace-mode-row">
        {heading}
        {inventoryMode === "archived" && (
          <button type="button" className="wiki-workspace-mode wiki-workspace-back" onClick={() => changeInventoryMode("folders")}>
            <CaretLeft aria-hidden="true" size={inSidebar ? 12 : 16} />
            <span>{t("pages.archive.back")}</span>
          </button>
        )}
        {!heading && inventoryMode === "folders" && <h2 className="wiki-workspace-mode wiki-workspace-title">{t("pages.folders.title")}</h2>}
        {/* The sidebar is narrow: its header keeps only what you act on. */}
        {!inSidebar && inventoryMode === "folders" && noteCount !== null && (
          <span className="wiki-workspace-note-count" title={t("pages.folders.noteCount", { count: noteCount })}>
            <span aria-hidden="true">{noteCount}</span>
            <span className="sr-only">{t("pages.folders.noteCount", { count: noteCount })}</span>
          </span>
        )}
        {/* One New button at the top of the list, as in Notion's sidebar: a page
            lands in the folder in view; a folder opens its name form. */}
        {inventoryMode === "folders" && (
          <NewMenu
            exampleWiki={exampleWiki}
            canCreateFolder={canCreateFolder}
            onNewPage={() => closeForNavigation(onCreatePage)(creationFolderPath ?? currentFolderPath ?? undefined)}
            // The New button has focus again, so the folder form returns it there.
            onNewFolder={() => { requestCreateFolder(); }}
          />
        )}
      </div>
      {inventoryMode === "folders" && !exampleWiki && <SpaceFoldersPrompt noteCount={noteCount} />}
      {inventoryMode === "archived" ? (
        <ArchivedNotesPanel currentPageId={currentPageId} onOpenPage={closeForNavigation(onOpenPage)} onRestored={onRestorePage} />
      ) : (
        <PageInventoryPanel
          inventoryScope={inventoryScope}
          currentPageId={currentPageId}
          currentFolderPath={creationFolderPath ?? currentFolderPath}
          fileTree
          browsing={browsing}
          filter={filter}
          onFilterChange={onFilterChange}
          onBrowse={closeForNavigation(onBrowse)}
          onCreatePage={closeForNavigation(onCreatePage)}
          onOpenDraft={closeForNavigation(onOpenDraft)}
          onOpenPage={closeForNavigation(onOpenPage)}
          onDeletePage={onDeletePage}
          onArchiveOpenPage={onArchiveOpenPage}
          onNoteCountChange={setNoteCount}
        />
      )}
      {children}
    </aside>
    </WikiFolderCreationContext.Provider>
  );
}

/** The directory's one New button: a small menu with New page and New folder. */
function NewMenu({ exampleWiki, canCreateFolder, onNewPage, onNewFolder }: {
  readonly exampleWiki: boolean;
  readonly canCreateFolder: boolean;
  readonly onNewPage: () => void;
  readonly onNewFolder: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuId = useId();
  useEffect(() => {
    if (!open) return;
    rootRef.current?.querySelector<HTMLElement>("[role='menuitem']:not([aria-disabled='true'])")?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);
  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  };
  const folderDisabled = !canCreateFolder;
  return (
    <div className="wiki-workspace-new-menu" ref={rootRef}>
      <button
        ref={buttonRef}
        type="button"
        className="wiki-workspace-new-folder wiki-workspace-new"
        aria-label={t("pages.folders.newMenu")}
        title={t("pages.folders.newMenu")}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen(value => !value)}
      >
        <Plus aria-hidden="true" size={16} />
      </button>
      {open && (
        <div
          id={menuId}
          className="mem-popover-surface notes-folder-context-menu wiki-workspace-new-popover"
          role="menu"
          aria-label={t("pages.folders.newMenu")}
          onKeyDown={event => {
            if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(true); }
            else if (event.key === "Tab") close(false);
            else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("[role='menuitem']"));
              const index = items.indexOf(document.activeElement as HTMLElement);
              if (items.length > 1) { event.preventDefault(); items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus(); }
            }
          }}
        >
          {!exampleWiki && (
            <button type="button" role="menuitem" onClick={() => { close(false); onNewPage(); }}>
              <NotePencil aria-hidden="true" size={16} />
              <span>{t("pages.overview.newPage")}</span>
            </button>
          )}
          <button type="button" role="menuitem" aria-disabled={folderDisabled} onClick={() => { if (folderDisabled) return; close(true); onNewFolder(); }}>
            <FolderPlus aria-hidden="true" size={16} />
            <span>{t("pages.folders.newFolder")}</span>
          </button>
        </div>
      )}
    </div>
  );
}
