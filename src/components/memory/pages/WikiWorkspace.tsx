// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Folder } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import type { Page } from "../../../lib/tauri";
import { WIKI_DIRECTORY_ID, WikiDirectory, type WikiInventoryMode } from "./WikiDirectory";
import type { WikiInventoryScope } from "./pageInventory";
import { NoteGroupFrame } from "./NoteGroupFrame";
import { useReportWorkspaceDemand, WIKI_LIST_DOCKED_WIDTH } from "../navigation/workspaceDemand";
import "./wikiWorkspace.css";

export interface WikiWorkspaceProps {
  readonly children: ReactNode;
  readonly tabs?: ReactNode;
  readonly onGlobalToolsHost?: (host: HTMLDivElement | null) => void;
  readonly onSharedReadingHost?: (host: HTMLDivElement | null) => void;
  readonly onSharedToolsHost?: (host: HTMLDivElement | null) => void;
  readonly onGroupFocus?: () => void;
  readonly secondaryOpen?: boolean;
  readonly onSecondaryHost?: (host: HTMLDivElement | null) => void;
  readonly inventoryScope?: WikiInventoryScope;
  readonly currentPageId?: string | null;
  readonly currentFolderPath?: string | null;
  readonly inventoryMode?: WikiInventoryMode;
  /** Increment to reveal the directory drawer (narrow windows), even if its mode is unchanged. */
  readonly directoryOpenRequest?: number;
  readonly onInventoryModeChange?: (mode: WikiInventoryMode) => void;
  readonly browsing?: boolean;
  readonly fullBleed?: boolean;
  /** A library with no notes and no drafts, or one whose getting-started
   *  checklist is still open. The directory has nothing to list or would read
   *  as a second sidebar, so the workspace collapses to the reading column
   *  alone and the start panel owns the whole width. */
  readonly directoryHidden?: boolean;
  /** The shell renders the directory (see Main): into this workspace's slot
   *  while it is a column here, or into the left sidebar. */
  readonly externalDirectory?: boolean;
  /** The directory sits in the left sidebar, so this workspace has no column for it. */
  readonly directoryInSidebar?: boolean;
  readonly onDirectoryHost?: (host: HTMLDivElement | null) => void;
  /** Increment to close the narrow directory drawer after a navigation. */
  readonly directoryCloseRequest?: number;
  readonly filter?: string;
  readonly onFilterChange?: (filter: string) => void;
  readonly onBrowse: (scope: WikiInventoryScope) => void;
  readonly onCreatePage: (folderPath?: string) => void;
  readonly onCreationFolderPathChange?: (path: string | null) => void;
  readonly onOpenDraft: (draftId: string, space: string | null) => void;
  readonly onOpenPage: (page: Page) => void;
  /** Called after a note was deleted from the directory so its tab closes. */
  readonly onDeletePage?: (pageId: string) => void;
  /** Hand an archive request to the editor that has this page open. Resolves
   *  true when that owner took it (or refused it as busy); false when no open
   *  editor owns the page and the list should archive it itself. */
  readonly onArchiveOpenPage?: (pageId: string) => Promise<boolean>;
  /** Called after an archived page was restored from the Archived pages list. */
  readonly onRestorePage?: (pageId: string) => void;
}

export function WikiWorkspace({
  children,
  tabs,
  onGroupFocus,
  onGlobalToolsHost,
  onSharedReadingHost,
  onSharedToolsHost,
  secondaryOpen,
  onSecondaryHost,
  inventoryScope = "all",
  currentPageId = null,
  currentFolderPath,
  inventoryMode: controlledInventoryMode,
  onInventoryModeChange,
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
  onArchiveOpenPage,
  onRestorePage,
  directoryOpenRequest = 0,
  externalDirectory = false,
  directoryInSidebar = false,
  onDirectoryHost,
  directoryCloseRequest = 0,
}: WikiWorkspaceProps) {
  const { t } = useTranslation();
  const [folderOpen, setFolderOpen] = useState(false);
  // Uncontrolled, the workspace still owns the mode: its column hides by it.
  const [localInventoryMode, setLocalInventoryMode] = useState<WikiInventoryMode>("folders");
  const inventoryMode = controlledInventoryMode ?? localInventoryMode;
  const changeInventoryMode = useCallback((mode: WikiInventoryMode) => {
    setLocalInventoryMode(mode);
    onInventoryModeChange?.(mode);
  }, [onInventoryModeChange]);
  const handledDirectoryRequest = useRef(directoryOpenRequest);
  useEffect(() => {
    if (handledDirectoryRequest.current === directoryOpenRequest) return;
    handledDirectoryRequest.current = directoryOpenRequest;
    setFolderOpen(true);
  }, [directoryOpenRequest]);
  const handledCloseRequest = useRef(directoryCloseRequest);
  useEffect(() => {
    if (handledCloseRequest.current === directoryCloseRequest) return;
    handledCloseRequest.current = directoryCloseRequest;
    setFolderOpen(false);
  }, [directoryCloseRequest]);
  const readingRootRef = useRef<HTMLDivElement | null>(null);
  const [sharedToolsElement, setSharedToolsElement] = useState<HTMLDivElement | null>(null);
  const setReadingRoot = useCallback((node: HTMLDivElement | null) => {
    readingRootRef.current = node;
    onSharedReadingHost?.(node);
  }, [onSharedReadingHost]);
  const setSharedToolsHost = useCallback((node: HTMLDivElement | null) => {
    setSharedToolsElement(node);
    onSharedToolsHost?.(node);
  }, [onSharedToolsHost]);
  useLayoutEffect(() => {
    const root = readingRootRef.current;
    if (!root || !sharedToolsElement) return;
    const update = () => root.style.setProperty("--wiki-shared-tools-width", `${sharedToolsElement.getBoundingClientRect().width}px`);
    update();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    observer?.observe(sharedToolsElement);
    window.addEventListener("resize", update);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", update);
      root.style.removeProperty("--wiki-shared-tools-width");
    };
  }, [sharedToolsElement]);
  const id = useId();
  useReportWorkspaceDemand("list", WIKI_LIST_DOCKED_WIDTH);
  const closeDrawer = useCallback(() => setFolderOpen(false), []);
  const openDrawer = useCallback(() => setFolderOpen(true), []);
  const hidden = directoryInSidebar || (directoryHidden && inventoryMode !== "archived");

  return (
    <div className="wiki-workspace" data-directory-hidden={hidden ? "true" : undefined}>
      <button
        type="button"
        className="wiki-workspace-folders-toggle"
        aria-expanded={folderOpen}
        aria-controls={externalDirectory ? WIKI_DIRECTORY_ID : `${id}-directory`}
        onClick={() => setFolderOpen(open => !open)}
      >
        <Folder aria-hidden="true" size={16} />
        <span>{t("sidebar.notes")}</span>
      </button>
      <div className={`wiki-workspace-body${folderOpen ? " is-folder-open" : ""}`}>
        {externalDirectory ? (
          <div className="wiki-workspace-directory-slot" ref={onDirectoryHost} />
        ) : (
          <WikiDirectory
            id={`${id}-directory`}
            inventoryScope={inventoryScope}
            currentPageId={currentPageId}
            currentFolderPath={currentFolderPath}
            inventoryMode={inventoryMode}
            onInventoryModeChange={changeInventoryMode}
            browsing={browsing}
            filter={filter}
            onFilterChange={onFilterChange}
            onBrowse={onBrowse}
            onCreatePage={onCreatePage}
            onCreationFolderPathChange={onCreationFolderPathChange}
            onOpenDraft={onOpenDraft}
            onOpenPage={onOpenPage}
            onDeletePage={onDeletePage}
            onArchiveOpenPage={onArchiveOpenPage}
            onRestorePage={onRestorePage}
            onNavigate={closeDrawer}
            onRevealRequest={openDrawer}
          />
        )}
        <div className={`wiki-workspace-reading${secondaryOpen ? " is-split" : ""}`} ref={setReadingRoot}>
          <div className={`wiki-primary-note-group${fullBleed ? " is-full-bleed" : ""}`}>
            <NoteGroupFrame id="primary" label={t("pages.groups.central")} tabs={tabs} onGlobalToolsHost={onGlobalToolsHost} contentId="wiki-note-content" onFocus={onGroupFocus}>
              {children}
            </NoteGroupFrame>
          </div>
          <div className="wiki-secondary-note-group" ref={onSecondaryHost} hidden={!secondaryOpen} />
          <div aria-label={t("pageInspector.label")} className="wiki-shared-tools-rail" data-tauri-drag-region={undefined}>
            <div className="note-group-document-tools wiki-shared-document-tools" ref={setSharedToolsHost} />
          </div>
        </div>
      </div>
    </div>
  );
}
