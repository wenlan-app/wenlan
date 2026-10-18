// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { CaretDown, Folder, FolderPlus } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import type { Page } from "../../../lib/tauri";
import { readWikiInventoryMode, writeWikiInventoryMode, type WikiInventoryMode } from "../../../lib/wikiNotesPreferences";
import { PageInventoryPanel } from "./PageInventoryPanel";
import { RecentNotesPanel } from "./RecentNotesPanel";
import { WikiFolderCreationContext } from "./WikiFolderCreationContext";
import type { WikiInventoryScope } from "./pageInventory";
import { NoteGroupFrame } from "./NoteGroupFrame";
import { useReportWorkspaceDemand, WIKI_LIST_DOCKED_WIDTH } from "../navigation/workspaceDemand";
import "./wikiWorkspace.css";

export interface WikiWorkspaceProps {
  readonly children: ReactNode;
  readonly tabs?: ReactNode;
  readonly onGlobalToolsHost?: (host: HTMLDivElement | null) => void;
  readonly onGroupFocus?: () => void;
  readonly secondaryOpen?: boolean;
  readonly onSecondaryHost?: (host: HTMLDivElement | null) => void;
  readonly inventoryScope?: WikiInventoryScope;
  readonly currentPageId?: string | null;
  readonly currentFolderPath?: string | null;
  readonly inventoryMode?: WikiInventoryMode;
  readonly onInventoryModeChange?: (mode: WikiInventoryMode) => void;
  readonly recentRevision?: number;
  readonly onOpenRecentPage?: (id: string) => void;
  readonly browsing?: boolean;
  readonly fullBleed?: boolean;
  /** A library with no notes and no drafts, or one whose getting-started
   *  checklist is still open. The directory has nothing to list or would read
   *  as a second sidebar, so the workspace collapses to the reading column
   *  alone and the start panel owns the whole width. */
  readonly directoryHidden?: boolean;
  readonly filter?: string;
  readonly onFilterChange?: (filter: string) => void;
  readonly onBrowse: (scope: WikiInventoryScope) => void;
  readonly onCreatePage: (folderPath?: string) => void;
  readonly onCreationFolderPathChange?: (path: string | null) => void;
  readonly onOpenDraft: (draftId: string, space: string | null) => void;
  readonly onOpenPage: (page: Page) => void;
  /** Called after a note was deleted from the directory so its tab closes. */
  readonly onDeletePage?: (pageId: string) => void;
}

export function WikiWorkspace({
  children,
  tabs,
  onGroupFocus,
  onGlobalToolsHost,
  secondaryOpen,
  onSecondaryHost,
  inventoryScope = "all",
  currentPageId = null,
  currentFolderPath,
  inventoryMode: controlledInventoryMode,
  onInventoryModeChange,
  recentRevision = 0,
  onOpenRecentPage,
  browsing = false,
  fullBleed = false,
  directoryHidden = false,
  filter,
  onFilterChange,
  onBrowse,
  onCreatePage,
  onCreationFolderPathChange,
  onOpenDraft,
  onOpenPage,
  onDeletePage,
}: WikiWorkspaceProps) {
  const { t } = useTranslation();
  const [folderOpen, setFolderOpen] = useState(false);
  const [localInventoryMode, setLocalInventoryMode] = useState<WikiInventoryMode>(readWikiInventoryMode);
  const inventoryMode = controlledInventoryMode ?? localInventoryMode;
  const [creationFolderPath, reportCreationFolderPath] = useState<string | null>(null);
  useEffect(() => {
    onCreationFolderPathChange?.(inventoryMode === "folders" ? creationFolderPath : null);
  }, [creationFolderPath, inventoryMode, onCreationFolderPathChange]);
  const [canCreateFolder, setCanCreateFolder] = useState(false);
  const [noteCount, setNoteCount] = useState<number | null>(null);
  const createFormRef = useRef<(() => void) | null>(null);
  const id = useId();
  useReportWorkspaceDemand("list", WIKI_LIST_DOCKED_WIDTH);
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
    setFolderOpen(true);
    createFormRef.current();
    return true;
  }, [canCreateFolder, inventoryMode]);
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
    setFolderOpen(false);
    callback(...args);
  };

  return (
    <WikiFolderCreationContext.Provider value={folderCreationContext}>
    <div className="wiki-workspace" data-directory-hidden={directoryHidden ? "true" : undefined}>
      <button
        type="button"
        className="wiki-workspace-folders-toggle"
        aria-expanded={folderOpen}
        aria-controls={`${id}-directory`}
        onClick={() => setFolderOpen(open => !open)}
      >
        <Folder aria-hidden="true" size={16} />
        <span>{t("sidebar.notes")}</span>
      </button>
      <div className={`wiki-workspace-body${folderOpen ? " is-folder-open" : ""}`}>
        <aside id={`${id}-directory`} className="wiki-workspace-directory">
          <div className="wiki-workspace-mode-row">
            <label className="wiki-workspace-mode">
              <span>{t("pages.recent.modeLabel")}</span>
              {/* Sizes the selector to the chosen mode's name so the note count can sit right after it. */}
              <span className="wiki-workspace-mode-sizer" aria-hidden="true">{t(inventoryMode === "recent" ? "pages.recent.title" : inventoryMode === "custom" ? "pages.recent.custom" : "pages.folders.title")}</span>
              <select
                aria-label={t("pages.recent.modeLabel")}
                title={isManualOrder ? t("pages.recent.customHint") : undefined}
                value={inventoryMode}
                onChange={event => changeInventoryMode(event.target.value as WikiInventoryMode)}
              >
                <option value="recent">{t("pages.recent.title")}</option>
                <option value="custom">{t("pages.recent.custom")}</option>
                <option value="folders">{t("pages.folders.title")}</option>
              </select>
              <CaretDown aria-hidden="true" size={16} />
            </label>
            {inventoryMode === "folders" && noteCount !== null && (
              <span className="wiki-workspace-note-count" title={t("pages.folders.noteCount", { count: noteCount })}>
                <span aria-hidden="true">{noteCount}</span>
                <span className="sr-only">{t("pages.folders.noteCount", { count: noteCount })}</span>
              </span>
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
          {inventoryMode !== "folders" ? (
            <RecentNotesPanel
              mode={inventoryMode}
              currentPageId={currentPageId}
              revision={recentRevision}
              onOpenPage={onOpenRecentPage ? closeForNavigation(onOpenRecentPage) : undefined}
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
              onNoteCountChange={setNoteCount}
            />
          )}
        </aside>
        <div className={`wiki-workspace-reading${secondaryOpen ? " is-split" : ""}`}>
          <div className={`wiki-primary-note-group${fullBleed ? " is-full-bleed" : ""}`}>
            <NoteGroupFrame id="primary" label={t("pages.groups.central")} tabs={tabs} onGlobalToolsHost={onGlobalToolsHost} contentId="wiki-note-content" onFocus={onGroupFocus}>
              {children}
            </NoteGroupFrame>
          </div>
          <div className="wiki-secondary-note-group" ref={onSecondaryHost} hidden={!secondaryOpen} />
        </div>
      </div>
    </div>
    </WikiFolderCreationContext.Provider>
  );
}
