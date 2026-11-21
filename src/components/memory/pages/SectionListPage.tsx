// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { CaretLeft } from "@phosphor-icons/react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { knowledgeFolderCreate, listSpaceRulesSuggestions, skillsFromLibraryEnabled, type Page } from "../../../lib/tauri";
import { confirmAction } from "../../../lib/confirmAction";
import { formatTimeAgo } from "../../../lib/dateFormat";
import { exampleWikiActive, useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { useTeamAccess } from "../../../web/teamAccess";
import { pinPage, unpinPage, usePinnedPages } from "../navigation/pinnedPages";
import { HOW_I_WORK_QUERY_KEYS, howIWorkPageTitle } from "../skills/howIWorkModel";
import { SpaceRulesAsk } from "../skills/SpaceRulesAsk";
import { RulesToolsHeader } from "../skills/RulesToolsHeader";
import { RulesSetupSettings } from "../skills/RulesSetupSettings";
import { useRulesSetup, useRulesSkillUpdates } from "../skills/useRulesSetup";
import { useScope } from "../team/scope";
import { archivePageFromList, canArchivePage } from "./archiveWikiPage";
import { deleteWikiPage } from "./deleteWikiPage";
import { pageFolderPath } from "./pageInventory";
import { scopedFolders } from "./PageInventoryPanel";
import { pageSpaceContext } from "./pagePresentation";
import { RULES_FOLDER_NAMES, SKILLS_FOLDER_NAMES, folderSection, sectionFolderPath, topFolderIsASpaces, type PageSection } from "./pageSections";
import { SpaceFoldersPrompt } from "./SpaceFoldersPrompt";
import { KNOWLEDGE_FOLDERS_QUERY_KEY, useKnowledgeFolders } from "./useKnowledgeFolders";
import { useSectionPages } from "./useSectionPages";
import "./sectionList.css";

export interface SectionListPageProps {
  readonly section: PageSection;
  /** Docs only: the folder shown, or null for the top of Docs. */
  readonly folderPath?: string | null;
  readonly onBrowseFolder: (folderPath: string | null) => void;
  readonly onCreatePage: (folderPath?: string) => void;
  readonly onOpenDraft: (draftId: string, space: string | null) => void;
  readonly onOpenPage: (page: Page) => void;
  /** Opens a page by id (a rule's source, or the page an accept made). */
  readonly onOpenPageId: (pageId: string) => void;
  readonly onDeletePage?: (pageId: string) => void;
  readonly onArchiveOpenPage?: (pageId: string) => Promise<boolean>;
  /** Skills only: find and install other people's skills. */
  readonly onFindSkills?: () => void;
  /** Tells the shell which folder a new page lands in (the tab strip's +). */
  readonly onCreationFolderPathChange?: (path: string | null) => void;
}

type RowMenu = { readonly pageId: string; readonly x: number; readonly y: number; readonly invoker: HTMLElement };

function changedAt(page: Page): number {
  const value = Date.parse(page.last_modified || page.last_compiled || page.created_at);
  return Number.isFinite(value) ? Math.floor(value / 1000) : 0;
}

/** The list page of one section, in the main area: for Docs, folders as
 *  cards and then pages with when each last changed; for Skills and Rules,
 *  their pages. The left side never lists pages itself (UX-FIND-4). */
export function SectionListPage({
  section,
  folderPath = null,
  onBrowseFolder,
  onCreatePage,
  onOpenDraft,
  onOpenPage,
  onOpenPageId,
  onDeletePage,
  onArchiveOpenPage,
  onFindSkills,
  onCreationFolderPathChange,
}: SectionListPageProps) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const id = useId();
  const queryClient = useQueryClient();
  const exampleWiki = useExampleWikiActive();
  const teamAccess = useTeamAccess();
  const member = teamAccess.member;
  const scope = useScope();
  const rulesSetup = useRulesSetup(section !== "docs" && !member);
  const skillUpdates = useRulesSkillUpdates(section === "skills" && !!rulesSetup?.adopted);
  const pinnedPages = usePinnedPages();
  const sections = useSectionPages(true);
  const folders = useKnowledgeFolders();
  const folderUnavailable = folders.isError || !!folders.data?.truncated;
  const folder = section === "docs" ? folderPath : null;
  const folderShown = useMemo(
    () => scopedFolders(sections.allPages, sections.pages, scope.teams.length > 0, Boolean(scope.team)),
    [sections.allPages, sections.pages, scope.teams.length, scope.team],
  );
  const sectionPages = sections.bySection[section];

  // Folders are cards only under Docs: a folder named Rules or Skills (and
  // what is in it) shows in its own section instead. A folder holding a
  // Docs page is always a card, whatever its name, so no page counted under
  // Docs is ever out of reach (a Space may itself be called Rules).
  const layout = sections.layout;
  const folderCards = useMemo(() => {
    if (section !== "docs" || folderUnavailable) return [];
    return (folders.data?.folders ?? [])
      .filter((item) => item.parent_path === (folder ?? "") && item.path !== (folder ?? "") && folderShown(item.path))
      .map((item) => ({
        ...item,
        count: sectionPages.filter((page) => {
          const path = pageFolderPath(page);
          return path === item.path || !!path?.startsWith(`${item.path}/`);
        }).length,
      }))
      .filter((item) => item.count > 0 || folderSection(item.path, layout) === "docs");
  }, [folder, folderShown, folderUnavailable, folders.data, layout, section, sectionPages]);

  const knownFolder = (path: string) => !!folders.data?.folders.some((item) => item.path === path);
  const rows = useMemo(() => {
    if (section !== "docs") return sectionPages;
    return sectionPages.filter((page) => {
      const path = pageFolderPath(page);
      if (folder !== null) return path === folder;
      // The top of Docs lists the pages in no folder, and every page while
      // folders cannot be read, so none is ever out of reach.
      return folderUnavailable || !path || !knownFolder(path);
    });
    // `knownFolder` reads `folders.data`, which is listed.
  }, [folder, folderUnavailable, folders.data, section, sectionPages]);

  // A new page lands in the folder in view; in Skills and Rules, in that
  // section's folder.
  const namedFolders = (folders.data?.folders ?? []).map((item) => item.path);
  const sectionFolder = section === "docs" ? folder : sectionFolderPath(section, namedFolders, layout);
  useEffect(() => {
    onCreationFolderPathChange?.(sectionFolder);
    return () => onCreationFolderPathChange?.(null);
  }, [onCreationFolderPathChange, sectionFolder]);

  const canWrite = !exampleWiki && !(member && teamAccess.writable.length === 0);
  const canCreateFolder = !exampleWiki && !member && !!folders.data && !folders.data.truncated;
  const [newOpen, setNewOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [createPending, setCreatePending] = useState(false);
  const [createError, setCreateError] = useState(false);
  const newRef = useRef<HTMLDivElement>(null);
  const newButtonRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!newOpen) return;
    newRef.current?.querySelector<HTMLElement>("[role='menuitem']:not([aria-disabled='true'])")?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!newRef.current?.contains(event.target as Node)) setNewOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [newOpen]);

  const newPage = async () => {
    if (section === "docs") { onCreatePage(folder ?? undefined); return; }
    if (sectionFolder !== null) { onCreatePage(sectionFolder); return; }
    // The section has no folder yet: make it, named as the section is, so
    // the page counts as a rule (or a skill) from its first save.
    const names = section === "rules" ? RULES_FOLDER_NAMES : SKILLS_FOLDER_NAMES;
    const label = t(section === "rules" ? "sidebar.rules" : "sidebar.skills");
    const folderName = names.includes(label) ? label : names[0];
    if (createPending) return;
    // Never somewhere else: a page made outside that folder would be a Docs
    // page, and a folder of that name inside a Space is that Space's alone.
    if (!canCreateFolder || topFolderIsASpaces(folderName, layout)) { toast(t("sections.noFolder", { section: label })); return; }
    setCreatePending(true);
    try {
      const made = await knowledgeFolderCreate("", folderName);
      void queryClient.invalidateQueries({ queryKey: KNOWLEDGE_FOLDERS_QUERY_KEY });
      onCreatePage(made.path);
    } catch {
      toast(t("sections.noFolder", { section: label }));
    } finally {
      setCreatePending(false);
    }
  };
  const createFolder = async (event: React.FormEvent) => {
    event.preventDefault();
    if (exampleWikiActive() || !name.trim() || createPending || !canCreateFolder) return;
    setCreatePending(true);
    setCreateError(false);
    try {
      const parentPath = folder ?? "";
      const result = await knowledgeFolderCreate(parentPath, name.trim());
      queryClient.setQueryData(KNOWLEDGE_FOLDERS_QUERY_KEY, { folders: [...(folders.data?.folders ?? []), { path: result.path, parent_path: parentPath, name: name.trim() }], truncated: false });
      setCreating(false);
      setName("");
      void queryClient.invalidateQueries({ queryKey: KNOWLEDGE_FOLDERS_QUERY_KEY });
    } catch {
      setCreateError(true);
    } finally {
      setCreatePending(false);
    }
  };
  const closeFolderForm = () => {
    setCreating(false);
    setCreateError(false);
    requestAnimationFrame(() => newButtonRef.current?.focus());
  };

  // A page row's own menu: open, pin, archive, delete (UX-FIND-6).
  const [menu, setMenu] = useState<RowMenu | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuPage = menu ? sectionPages.find((page) => page.id === menu.pageId) : undefined;
  const busyRef = useRef(false);
  useLayoutEffect(() => {
    if (!menu) return;
    const element = menuRef.current;
    if (!element) return;
    const bounds = element.getBoundingClientRect();
    const x = Math.max(8, Math.min(menu.x, window.innerWidth - bounds.width - 8));
    const y = Math.max(8, Math.min(menu.y, window.innerHeight - bounds.height - 8));
    if (x !== menu.x || y !== menu.y) setMenu((current) => current ? { ...current, x, y } : current);
    element.querySelector<HTMLElement>("[role='menuitem']")?.focus();
    const dismiss = () => setMenu(null);
    const onPointerDown = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) dismiss();
    };
    document.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", dismiss, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", dismiss, true);
    };
  }, [menu]);
  const openRow = (page: Page) => {
    if (page.status === "draft") onOpenDraft(page.id, pageSpaceContext(page) ?? null);
    else onOpenPage(page);
  };
  const deleteRow = async (page: Page) => {
    if (member || busyRef.current || exampleWikiActive()) return;
    busyRef.current = true;
    const confirmed = await confirmAction(t("pageDetail.deleteConfirm"), {
      confirm: t("pageDetail.deletePage"),
      cancel: t("common.cancel"),
    });
    // The example may have opened while the question was up.
    if (!confirmed || exampleWikiActive()) { busyRef.current = false; return; }
    try {
      await deleteWikiPage(queryClient, page);
      onDeletePage?.(page.id);
    } catch {
      toast(t("pageDetail.deleteError"));
    } finally {
      busyRef.current = false;
    }
  };
  const openMenuAt = (page: Page, invoker: HTMLElement, x: number, y: number) => setMenu({ pageId: page.id, invoker, x, y });

  // Rules the AI drew from pages and nobody answered yet stay in their own
  // box here; they count only after Accept (UX-SKILL-1).
  const rulesFeature = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.enabled,
    queryFn: skillsFromLibraryEnabled,
    enabled: section === "rules" && !exampleWiki,
    staleTime: 30_000,
    retry: false,
  });
  const pendingRules = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.spaceSuggestions,
    queryFn: listSpaceRulesSuggestions,
    enabled: section === "rules" && rulesFeature.data === true,
    retry: false,
  });
  const pending = section === "rules" ? pendingRules.data ?? [] : [];

  const sectionTitle = t(section === "docs" ? "sidebar.pages" : section === "skills" ? "sidebar.skills" : "sidebar.rules");
  const folderName = folder === null ? null : folder.split("/").pop() ?? folder;
  const parentPath = folder === null ? null : folder.includes("/") ? folder.slice(0, folder.lastIndexOf("/")) : null;
  const parentName = parentPath === null ? sectionTitle : parentPath.split("/").pop() ?? parentPath;
  const title = folderName ?? sectionTitle;
  // Adopted skills live in tool folders. An update still gets a row when
  // there is no corresponding Skills page in the library.
  const pageBackedUpdates = new Set(rows.filter((page) => page.status === "active").map((page) => page.title));
  const localUpdates = [...(skillUpdates ?? [])].filter((name) => !pageBackedUpdates.has(name));
  const empty = sections.loaded && rows.length === 0 && folderCards.length === 0 && localUpdates.length === 0;
  const menuItems = !canWrite ? [] : [
    ...(section === "docs" && !member ? ["folder" as const] : []),
    // A team member's browser has no skills of its own to hold: the entry
    // shows wherever Wenlan is the person's own.
    ...(section === "skills" && onFindSkills && !member ? ["find" as const] : []),
  ];

  return (
    <section className="section-list" data-section={section} data-testid="section-list" data-keeps-group-focus="" aria-busy={sections.pending || (section === "docs" && folders.isPending)} aria-labelledby={`${id}-title`}>
      {folder !== null && (
        <button type="button" className="section-list-back" onClick={() => onBrowseFolder(parentPath)}>
          <CaretLeft aria-hidden="true" size={12} />
          <span>{parentName}</span>
        </button>
      )}
      <header className="section-list-header">
        <h1 id={`${id}-title`}>{title}</h1>
        {canWrite && (
          <div className="section-list-new" ref={newRef}>
            <button
              ref={newButtonRef}
              type="button"
              className="section-list-new-button"
              disabled={createPending}
              aria-haspopup={menuItems.length > 0 ? "menu" : undefined}
              aria-expanded={menuItems.length > 0 ? newOpen : undefined}
              onClick={() => { if (menuItems.length > 0) setNewOpen((open) => !open); else void newPage(); }}
            >
              {t("sections.new")}
            </button>
            {newOpen && (
              <div
                className="mem-popover-surface notes-folder-context-menu section-list-new-menu"
                role="menu"
                aria-label={t("sections.new")}
                onKeyDown={(event) => {
                  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setNewOpen(false); newButtonRef.current?.focus(); }
                  else if (event.key === "Tab") setNewOpen(false);
                  else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                    const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("[role='menuitem']"));
                    const index = items.indexOf(document.activeElement as HTMLElement);
                    if (items.length > 1) { event.preventDefault(); items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus(); }
                  }
                }}
              >
                <button type="button" role="menuitem" onClick={() => { setNewOpen(false); void newPage(); }}>{t("pages.overview.newPage")}</button>
                {menuItems.includes("folder") && (
                  <button type="button" role="menuitem" aria-disabled={!canCreateFolder} onClick={() => { if (!canCreateFolder) return; setNewOpen(false); setCreating(true); setCreateError(false); }}>{t("pages.folders.newFolder")}</button>
                )}
                {menuItems.includes("find") && (
                  <button type="button" role="menuitem" onClick={() => { setNewOpen(false); onFindSkills?.(); }}>{t("findSkills.entry")}</button>
                )}
              </div>
            )}
          </div>
        )}
      </header>

      {rulesSetup && (section === "rules" ? <RulesSetupSettings />
        : <RulesToolsHeader delivers setupOnly skillUpdates={skillUpdates?.size} />)}
      {section === "docs" && folder === null && !exampleWiki && <SpaceFoldersPrompt noteCount={sections.loaded ? sections.pages.length : null} />}
      {pending.map((item) => <SpaceRulesAsk key={item.space_id} space={{ id: item.space_id, name: item.space_name }} onOpenPage={onOpenPageId} variant="pending" />)}

      {creating && (
        <form className="notes-folder-create section-list-folder-form" onSubmit={createFolder} onKeyDown={(event) => { if (event.key === "Escape" && !createPending) { event.preventDefault(); event.stopPropagation(); closeFolderForm(); } }}>
          <label htmlFor={`${id}-name`}>{t("pages.folders.folderName")}</label>
          <input id={`${id}-name`} autoFocus value={name} disabled={createPending} maxLength={100} onChange={(event) => setName(event.target.value)} />
          <div>
            <button type="submit" disabled={createPending || !name.trim()}>{t("pages.folders.create")}</button>
            <button type="button" onClick={closeFolderForm} disabled={createPending}>{t("pages.folders.cancel")}</button>
          </div>
          {createError && <p role="alert">{t("pages.folders.createError")}</p>}
        </form>
      )}

      {sections.pending ? (
        <p className="section-list-state" role="status">{t("pages.overview.loading")}</p>
      ) : sections.failed ? (
        <div className="section-list-state" role="alert">
          <p>{t("pages.overview.error")}</p>
          <button type="button" onClick={sections.retry}>{t("pageDetail.retry")}</button>
        </div>
      ) : (
        <>
          {folderCards.length > 0 && (
            <>
              <h2 className="section-list-label" id={`${id}-folders`}>{t("pages.folders.title")}</h2>
              <ul className="section-list-folders" aria-labelledby={`${id}-folders`}>
                {folderCards.map((card) => (
                  <li key={card.path} data-folder-path={card.path}>
                    <button type="button" className="section-list-folder" aria-label={card.name} aria-description={t("pages.folders.noteCount", { count: card.count })} onClick={() => onBrowseFolder(card.path)}>
                      <span className="section-list-folder-name">{card.name}</span>
                      <span aria-hidden="true" className="section-list-folder-count">{card.count}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}
          {(rows.length > 0 || localUpdates.length > 0) && (
            <>
              {section === "docs" && <h2 className="section-list-label" id={`${id}-pages`}>{t("sections.pages")}</h2>}
              <ul className="section-list-pages" aria-labelledby={section === "docs" ? `${id}-pages` : `${id}-title`}>
                {rows.map((page) => {
                  const draft = page.status === "draft";
                  const newVersion = page.status === "active" && skillUpdates?.has(page.title);
                  const rowTitle = draft && !page.title.trim() ? t("pages.overview.untitledDraft") : howIWorkPageTitle(page, t);
                  const at = changedAt(page);
                  return (
                    <li key={page.id} data-page-id={page.id} data-page-status={page.status} data-folder-path={pageFolderPath(page) ?? ""}>
                      <button
                        type="button"
                        className="section-list-page"
                        aria-label={newVersion ? t("rulesSetup.openUpdatedSkill", { title: rowTitle }) : t("pages.overview.openPage", { title: rowTitle })}
                        title={rowTitle}
                        onClick={() => openRow(page)}
                        onContextMenu={(event) => { event.preventDefault(); openMenuAt(page, event.currentTarget, event.clientX, event.clientY); }}
                        onKeyDown={(event) => {
                          if (event.key !== "ContextMenu" && !(event.key === "F10" && event.shiftKey)) return;
                          event.preventDefault();
                          const bounds = event.currentTarget.getBoundingClientRect();
                          openMenuAt(page, event.currentTarget, bounds.left, bounds.bottom);
                        }}
                      >
                        <span className="section-list-page-title">{rowTitle}</span>
                        {newVersion && <span className="rules-setup-skill-update">{t("rulesSetup.newVersion")}</span>}
                        {at > 0 && <time className="section-list-page-time" dateTime={new Date(at * 1000).toISOString()}>{formatTimeAgo(at, locale)}</time>}
                      </button>
                    </li>
                  );
                })}
                {localUpdates.map((name) => (
                  <li key={name} className="section-list-page rules-setup-skill-row">
                    <span className="section-list-page-title">{name}</span>
                    <span className="rules-setup-skill-update">{t("rulesSetup.newVersion")}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
          {empty && <p className="section-list-state section-list-empty">{t(folder !== null ? "pages.folders.empty" : `sections.empty.${section}`)}</p>}
          {folderUnavailable && section === "docs" && <p className="section-list-state" role="status">{t("pages.folders.unavailable")}</p>}
        </>
      )}

      {menu && createPortal(
        <div
          className="mem-popover-surface notes-folder-context-menu"
          role="menu"
          aria-label={menuPage ? howIWorkPageTitle(menuPage, t) : undefined}
          ref={menuRef}
          style={{ left: menu.x, top: menu.y }}
          onKeyDown={(event) => {
            if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); menu.invoker.focus(); setMenu(null); }
            else if (event.key === "Tab") setMenu(null);
            else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("[role='menuitem']"));
              const index = items.indexOf(document.activeElement as HTMLElement);
              if (items.length > 1) { event.preventDefault(); items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus(); }
            }
          }}
        >
          <button type="button" role="menuitem" aria-disabled={!menuPage} onClick={() => { if (!menuPage) return; setMenu(null); openRow(menuPage); }}>{t("pages.folders.openNote")}</button>
          {!exampleWiki && !scope.team && menuPage?.status === "active" && (
            <button type="button" role="menuitem" onClick={() => { const { id: pageId, title: pageTitle } = menuPage; setMenu(null); if (pinnedPages.some((pinned) => pinned.id === pageId)) unpinPage(pageId); else pinPage({ id: pageId, title: pageTitle }); }}>
              {t(pinnedPages.some((pinned) => pinned.id === menuPage.id) ? "sidebar.unpinPage" : "sidebar.pinPage")}
            </button>
          )}
          {!exampleWiki && !member && canArchivePage(menuPage) && (
            <button type="button" role="menuitem" onClick={() => { if (!menuPage) return; const { invoker } = menu; setMenu(null); void archivePageFromList(queryClient, menuPage, { t, onArchiveOpenPage }).then((archived) => { if (!archived) invoker.focus(); }); }}>{t("pageDetail.archivePage")}</button>
          )}
          {!exampleWiki && !member && (
            <button type="button" role="menuitem" className="is-danger" aria-disabled={!menuPage} onClick={() => { if (!menuPage) return; setMenu(null); void deleteRow(menuPage); }}>{t("pages.folders.deleteNote")}</button>
          )}
        </div>,
        document.body,
      )}
    </section>
  );
}
