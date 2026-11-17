// SPDX-License-Identifier: AGPL-3.0-only
import { forwardRef, useEffect, useRef } from "react";
import { GearSix, SidebarSimple, SignOut } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import IdentityCard from "./IdentityCard";
import { PrimaryNavigation, SIDEBAR_RECENT_LISTED } from "./navigation/PrimaryNavigation";
import { sectionCounts, useRecentSectionPages, useSectionPages } from "./pages/useSectionPages";
import { howIWorkPageTitle } from "./skills/howIWorkModel";
import { ReviewEnvironmentBadge } from "./navigation/ReviewEnvironmentBadge";
import { SearchEntry } from "./SearchEntry";
import ActivityStatus from "./activity/ActivityStatus";
import type { RegisteredSource } from "../../lib/tauri";
import ServiceStatus from "./ServiceStatus";
import type { GlobalNavigation } from "./navigation/viewState";
import { movePinnedPage, unpinPage, usePinnedPages } from "./navigation/pinnedPages";
import { ICONS_SIDEBAR_WIDTH, type SidebarMode } from "./navigation/navigationPreferences";
import "./navigation/notes-sidebar.css";
import { isWebBuild } from "../../web/runtime";
import { forgetTeamKey, isTeamMemberMode } from "../../web/team";

interface SidebarProps {
  readonly activeNavigation?: GlobalNavigation | null;
  readonly hidden: boolean;
  readonly mode: SidebarMode;
  readonly onEntityClick: (entityId: string) => void;
  readonly onNavigateEntities?: () => void;
  readonly onNavigateGraph?: () => void;
  readonly onNavigateActivity?: () => void;
 /** Opens a source Activity reports as unreachable. */
 onOpenSource?: (source: RegisteredSource | null) => void;
  readonly activityCurrent?: boolean;
  /** Opens the connect flow from "What your AI can do" when no tool is connected. */
  readonly onConnectTool?: () => void;
  readonly onNavigateHome?: () => void;
  readonly onNavigatePages?: () => void;
  readonly onNavigateSkills?: () => void;
  readonly onNavigateRules?: () => void;
  readonly onNavigateSettings?: () => void;
  readonly onNavigateSpaces?: (create: boolean) => void;
  /** The Private / team switch, above everything; only for someone on a team. */
  readonly switcher?: (compact: boolean) => React.ReactNode;
  readonly onOpenAbout?: () => void;
  readonly onOpenSearch?: (trigger: HTMLButtonElement) => void;
  readonly searchOpen?: boolean;
  readonly searchDisabled?: boolean;
  readonly onRequestClose?: () => void;
  readonly open?: boolean;
  readonly presentation?: "desktop" | "overlay";
  /** Opens a pinned or recently opened page; without it neither list shows. */
  readonly onOpenPinnedPage?: (pageId: string) => void;
  readonly activePageId?: string | null;
}

function closeAfterNavigation<Arguments extends readonly unknown[]>(
  navigate: (...arguments_: Arguments) => void,
  close: (() => void) | undefined,
): (...arguments_: Arguments) => void;
function closeAfterNavigation<Arguments extends readonly unknown[]>(
  navigate: ((...arguments_: Arguments) => void) | undefined,
  close: (() => void) | undefined,
): ((...arguments_: Arguments) => void) | undefined;
function closeAfterNavigation<Arguments extends readonly unknown[]>(
  navigate: ((...arguments_: Arguments) => void) | undefined,
  close: (() => void) | undefined,
): ((...arguments_: Arguments) => void) | undefined {
  if (!navigate) return undefined;
  return (...arguments_: Arguments) => {
    navigate(...arguments_);
    close?.();
  };
}

export default function Sidebar({
  activeNavigation = null,
  hidden,
  mode,
  onEntityClick,
  onNavigateEntities,
  onNavigateGraph,
  onNavigateActivity,
  onOpenSource,
  activityCurrent = false,
  onConnectTool,
  onNavigateHome,
  onNavigatePages,
  onNavigateSkills,
  onNavigateRules,
  onNavigateSettings,
  onNavigateSpaces = () => {},
  switcher,
  onOpenAbout,
  onOpenSearch,
  searchOpen = false,
  searchDisabled = false,
  onRequestClose,
  open = !hidden,
  presentation = "desktop",
  onOpenPinnedPage,
  activePageId = null,
}: SidebarProps) {
  const web = isWebBuild();
  const teamMember = isTeamMemberMode();
  const { t } = useTranslation();
  const pinnedPages = usePinnedPages();
  const sections = useSectionPages();
  const recentPages = useRecentSectionPages(sections.pages, SIDEBAR_RECENT_LISTED)
    .map((recent) => ({ ...recent, title: howIWorkPageTitle(recent.page, t) }));
  const asideRef = useRef<HTMLElement>(null);
  const overlay = presentation === "overlay";
  const compact = !overlay && mode === "icons";
  const closeOverlay = overlay ? onRequestClose : undefined;
  const sidebarVisible = overlay ? open : !hidden;

  useEffect(() => {
    if (!overlay || !open) return;
    const first = asideRef.current?.querySelector<HTMLElement>("button, [href], input, select, textarea, [tabindex]:not([tabindex='-1'])");
    first?.focus();
  }, [open, overlay]);

  const trapFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== "Tab" || !overlay) return;
    const focusable = asideRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])");
    if (!focusable || focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <>
      {overlay && open && (
        <button
          aria-label={t("sidebar.close")}
          className="fixed inset-x-0 bottom-0 top-[52px] z-30 border-0 bg-black/40"
          onClick={onRequestClose}
          type="button"
        />
      )}
      <aside
        aria-hidden={!sidebarVisible}
        aria-label={t("sidebar.navigation")}
        className="memory-sidebar notes-workspace-sidebar"
        data-collapsed={hidden}
        data-open={sidebarVisible}
        data-presentation={presentation}
        data-sidebar-overlay={overlay && open ? "true" : undefined}
        inert={!sidebarVisible}
        onKeyDown={trapFocus}
        ref={asideRef}
        style={{
          backgroundColor: "var(--workspace-sidebar-background, var(--mem-sidebar))",
          borderRight: "none",
          boxSizing: "border-box",
          bottom: overlay ? 0 : undefined,
          height: overlay ? "auto" : "100%",
          left: overlay ? 0 : undefined,
          position: overlay ? "fixed" : "relative",
          top: overlay ? 52 : undefined,
          transform: overlay && !open ? "translateX(-100%)" : undefined,
          visibility: !sidebarVisible ? "hidden" : "visible",
          width: overlay ? 240 : hidden ? 0 : compact ? ICONS_SIDEBAR_WIDTH : "var(--workspace-sidebar-width, 240px)",
          zIndex: overlay ? 40 : 2,
        }}
      >
        {!web && switcher?.(compact)}
        {onOpenSearch && (
          <div className={`notes-sidebar-search${compact ? " notes-sidebar-search--compact" : ""}`}>
            <SearchEntry compact={compact} disabled={searchDisabled} expanded={searchOpen} onOpen={onOpenSearch} />
          </div>
        )}
        <div className="notes-icon-rail">
          <PrimaryNavigation
            active={activeNavigation}
            compact={compact}
            labels={{
              entities: t("sidebar.entities"),
              graph: t("sidebar.graph"),
              home: t("sidebar.home"),
              navigation: t("sidebar.navigation"),
              pages: t("sidebar.pages"),
              skills: t("sidebar.skills"),
              rules: t("sidebar.rules"),
              spaces: t("sidebar.spaces"),
              pinned: t("sidebar.pinnedPages"),
              recent: t("sidebar.recentPages"),
              counted: (label, count) => t("sidebar.sectionCount", { label, count }),
              unpin: (title) => t("sidebar.unpinNamed", { title }),
            }}
            counts={sectionCounts(sections)}
            recentPages={recentPages}
            pinnedPages={pinnedPages}
            activePinnedPageId={activePageId}
            onOpenPinnedPage={closeAfterNavigation(onOpenPinnedPage, closeOverlay)}
            onUnpinPage={unpinPage}
            onMovePinnedPage={movePinnedPage}
            onNavigateEntities={teamMember ? undefined : closeAfterNavigation(onNavigateEntities, closeOverlay)}
            onNavigateGraph={teamMember ? undefined : closeAfterNavigation(onNavigateGraph, closeOverlay)}
            onNavigateHome={closeAfterNavigation(onNavigateHome, closeOverlay)}
            onNavigatePages={closeAfterNavigation(onNavigatePages, closeOverlay)}
            onNavigateSkills={closeAfterNavigation(onNavigateSkills, closeOverlay)}
            onNavigateRules={closeAfterNavigation(onNavigateRules, closeOverlay)}
            onNavigateSpaces={closeAfterNavigation(onNavigateSpaces, closeOverlay)}
          />
        </div>
        <div className={`notes-rail-utilities${compact ? " notes-rail-utilities--icons" : ""}`}>
          {/* The service speaks in Activity's place, so the footer keeps its rows. */}
          {sidebarVisible && !web && (
            <ServiceStatus compact={compact}>
              {onNavigateActivity && (
                <ActivityStatus
                  compact={compact}
                  current={activityCurrent}
                  onOpenActivity={closeAfterNavigation(onNavigateActivity, closeOverlay)}
                  onOpenSource={closeAfterNavigation(onOpenSource, closeOverlay)}
                />
              )}
            </ServiceStatus>
          )}
          <ReviewEnvironmentBadge compact />
          {web && onNavigateSettings && (
            <button
              aria-label={t("identityCard.settings")}
              className={compact ? "notes-rail-button" : "notes-primary-link"}
              onClick={closeAfterNavigation(onNavigateSettings, closeOverlay)}
              title={compact ? t("identityCard.settings") : undefined}
              type="button"
            >
              <span aria-hidden="true" className="notes-navigation-glyph"><GearSix /></span>
              {!compact && <span>{t("identityCard.settings")}</span>}
            </button>
          )}
          {teamMember && (
            <button
              aria-label={t("teamWeb.forget")}
              className={compact ? "notes-rail-button" : "notes-primary-link"}
              data-testid="team-web-forget"
              onClick={forgetTeamKey}
              title={compact ? t("teamWeb.forget") : undefined}
              type="button"
            >
              <span aria-hidden="true" className="notes-navigation-glyph"><SignOut /></span>
              {!compact && <span>{t("teamWeb.forget")}</span>}
            </button>
          )}
          {!web && <IdentityCard
            compact={compact}
            onOpenDetail={closeAfterNavigation(onEntityClick, closeOverlay)}
            onOpenSettings={closeAfterNavigation(onNavigateSettings, closeOverlay)}
            onOpenAbout={closeAfterNavigation(onOpenAbout, closeOverlay)}
            onConnectTool={closeAfterNavigation(onConnectTool, closeOverlay)}
          />}
        </div>
      </aside>
    </>
  );
}

/** Sidebar toggle button for use in the header. */
export const SidebarToggleButton = forwardRef<HTMLButtonElement, { readonly collapsed: boolean; readonly onToggle: () => void }>(function SidebarToggleButton({ collapsed, onToggle }, ref) {
  const { t } = useTranslation();
  return (
    <button
      aria-label={collapsed ? t("sidebar.show") : t("sidebar.hide")}
      data-sidebar-toggle="true"
      aria-expanded={!collapsed}
      ref={ref}
      onClick={onToggle}
      className="mem-icon-action workspace-panel-toggle"
      title={collapsed ? t("sidebar.show") : t("sidebar.hide")}
      type="button"
    >
      <SidebarSimple aria-hidden="true" size={18} />
    </button>
  );
});
