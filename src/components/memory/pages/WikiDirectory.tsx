// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { CaretDown, FolderPlus, NotePencil } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import type { Page } from "../../../lib/tauri";
import { readWikiInventoryMode, writeWikiInventoryMode, type WikiInventoryMode } from "../../../lib/wikiNotesPreferences";
import { ArchivedNotesPanel } from "./ArchivedNotesPanel";
import { PageInventoryPanel } from "./PageInventoryPanel";
import { RecentNotesPanel } from "./RecentNotesPanel";
import { WikiFolderCreationContext } from "./WikiFolderCreationContext";
import type { WikiInventoryScope } from "./pageInventory";

/** The id of the one directory the shell renders (see Main). */
export const WIKI_DIRECTORY_ID = "wiki-directory";

export interface WikiDirectoryProps {
  readonly id?: string;
  /** Where the directory sits: its own column in the Wiki workspace, or a
   *  section of the app's one left sidebar, below the destinations. */
  readonly placement?: "workspace" | "sidebar";
  /** Leads the mode row in the sidebar: the Wiki destination itself. */
  readonly heading?: ReactNode;
  readonly inventoryScope?: WikiInventoryScope;
  readonly currentPageId?: string | null;
  readonly currentFolderPath?: string | null;
  readonly inventoryMode?: WikiInventoryMode;
  readonly onInventoryModeChange?: (mode: WikiInventoryMode) => void;
  readonly recentRevision?: number;
  readonly onOpenRecentPage?: (id: string) => void;
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

/** Every page of the Wiki: by folder, recent, hand-ordered or archived. */
export function WikiDirectory({
  id,
  placement = "workspace",
  heading,
  inventoryScope = "all",
  currentPageId = null,
  currentFolderPath,
  inventoryMode: controlledInventoryMode,
  onInventoryModeChange,
  recentRevision = 0,
  onOpenRecentPage,
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
  const [localInventoryMode, setLocalInventoryMode] = useState<WikiInventoryMode>(readWikiInventoryMode);
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
  const isManualOrder = inventoryMode === "custom";
  const changeInventoryMode = (mode: WikiInventoryMode) => {
    writeWikiInventoryMode(mode);
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
        <label className="wiki-workspace-mode">
          <span>{t("pages.recent.modeLabel")}</span>
          {/* Sizes the selector to the chosen mode's name so the note count can sit right after it. */}
          <span className="wiki-workspace-mode-sizer" aria-hidden="true">{t(inventoryMode === "recent" ? "pages.recent.title" : inventoryMode === "custom" ? "pages.recent.custom" : inventoryMode === "archived" ? "pages.archive.title" : "pages.folders.title")}</span>
          <select
            aria-label={t("pages.recent.modeLabel")}
            title={isManualOrder ? t("pages.recent.customHint") : undefined}
            value={inventoryMode}
            onChange={event => changeInventoryMode(event.target.value as WikiInventoryMode)}
          >
            <option value="recent">{t("pages.recent.title")}</option>
            <option value="custom">{t("pages.recent.custom")}</option>
            <option value="folders">{t("pages.folders.title")}</option>
            <option value="archived">{t("pages.archive.title")}</option>
          </select>
          <CaretDown aria-hidden="true" size={inSidebar ? 12 : 16} />
        </label>
        {/* The sidebar is narrow: the mode row keeps only what you act on. */}
        {!inSidebar && inventoryMode === "folders" && noteCount !== null && (
          <span className="wiki-workspace-note-count" title={t("pages.folders.noteCount", { count: noteCount })}>
            <span aria-hidden="true">{noteCount}</span>
            <span className="sr-only">{t("pages.folders.noteCount", { count: noteCount })}</span>
          </span>
        )}
        {/* New page sits at the top of the list, as in Obsidian's file
            explorer; in Folders it lands in the folder in view. */}
        {inventoryMode !== "archived" && !exampleWiki && (
          <button
            type="button"
            className="wiki-workspace-new-folder wiki-workspace-new-page"
            aria-label={t("pages.overview.newPage")}
            title={t("pages.overview.newPage")}
            onClick={() => closeForNavigation(onCreatePage)(inventoryMode === "folders" ? creationFolderPath ?? currentFolderPath ?? undefined : undefined)}
          >
            <NotePencil aria-hidden="true" size={16} />
          </button>
        )}
        {inventoryMode === "folders" && (
          <button
            type="button"
            className="wiki-workspace-new-folder"
            aria-label={t("pages.folders.newFolder")}
            title={t("pages.folders.newFolder")}
            disabled={!canCreateFolder}
            onClick={event => { event.currentTarget.focus(); requestCreateFolder(); }}
          >
            <FolderPlus aria-hidden="true" size={16} />
          </button>
        )}
      </div>
      {inventoryMode === "archived" ? (
        <ArchivedNotesPanel currentPageId={currentPageId} onOpenPage={closeForNavigation(onOpenPage)} onRestored={onRestorePage} />
      ) : inventoryMode !== "folders" ? (
        <RecentNotesPanel
          mode={inventoryMode}
          currentPageId={currentPageId}
          revision={recentRevision}
          onOpenPage={onOpenRecentPage ? closeForNavigation(onOpenRecentPage) : undefined}
          onDeletePage={onDeletePage}
          onArchiveOpenPage={onArchiveOpenPage}
          onBrowseFolders={() => changeInventoryMode("folders")}
        />
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
