// SPDX-License-Identifier: AGPL-3.0-only
import { forwardRef, useEffect, useRef } from "react";
import { SidebarSimple } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import IdentityCard from "./IdentityCard";
import { PrimaryNavigation } from "./navigation/PrimaryNavigation";
import { ReviewEnvironmentBadge } from "./navigation/ReviewEnvironmentBadge";
import { SearchEntry } from "./SearchEntry";
import ActivityStatus from "./activity/ActivityStatus";
import ServiceStatus from "./ServiceStatus";
import { AskAiEntry } from "./askAi/AskAiEntry";
import type { GlobalNavigation } from "./navigation/viewState";
import { ICONS_SIDEBAR_WIDTH, type SidebarMode } from "./navigation/navigationPreferences";
import "./navigation/notes-sidebar.css";

interface SidebarProps {
  readonly activeNavigation?: GlobalNavigation | null;
  readonly hidden: boolean;
  readonly mode: SidebarMode;
  readonly onEntityClick: (entityId: string) => void;
  readonly onNavigateEntities?: () => void;
  readonly onNavigateGraph?: () => void;
  readonly onNavigateLog?: () => void;
  readonly onNavigateActivity?: () => void;
  readonly activityCurrent?: boolean;
  /** Opens the connect flow from "What your AI can do" when no tool is connected. */
  readonly onConnectTool?: () => void;
  readonly onNavigatePages?: () => void;
  readonly onNavigateSettings?: () => void;
  readonly onNavigateSources?: () => void;
  readonly onNavigateSpaces?: (create: boolean) => void;
  readonly onOpenAbout?: () => void;
  readonly onOpenSearch?: (trigger: HTMLButtonElement) => void;
  readonly searchOpen?: boolean;
  readonly searchDisabled?: boolean;
  readonly onRequestClose?: () => void;
  readonly open?: boolean;
  readonly presentation?: "desktop" | "overlay";
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
  onNavigateLog,
  onNavigateActivity,
  activityCurrent = false,
  onConnectTool,
  onNavigatePages,
  onNavigateSettings,
  onNavigateSources,
  onNavigateSpaces = () => {},
  onOpenAbout,
  onOpenSearch,
  searchOpen = false,
  searchDisabled = false,
  onRequestClose,
  open = !hidden,
  presentation = "desktop",
}: SidebarProps) {
  const { t } = useTranslation();
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
              memories: t("sidebar.memories"),
              more: t("sidebar.more"),
              navigation: t("sidebar.navigation"),
              pages: t("sidebar.pages"),
              pinToSidebar: (name) => t("sidebar.pinToSidebar", { name }),
              sources: t("sidebar.sources"),
              spaces: t("sidebar.spaces"),
              unpinFromSidebar: (name) => t("sidebar.unpinFromSidebar", { name }),
            }}
            onNavigateEntities={closeAfterNavigation(onNavigateEntities, closeOverlay)}
            onNavigateGraph={closeAfterNavigation(onNavigateGraph, closeOverlay)}
            onNavigateLog={closeAfterNavigation(onNavigateLog, closeOverlay)}
            onNavigatePages={closeAfterNavigation(onNavigatePages, closeOverlay)}
            onNavigateSources={closeAfterNavigation(onNavigateSources, closeOverlay)}
            onNavigateSpaces={closeAfterNavigation(onNavigateSpaces, closeOverlay)}
          />
        </div>
        <div className={`notes-rail-utilities${compact ? " notes-rail-utilities--icons" : ""}`}>
          {/* First in the group, so the AI and Activity entries keep their places. */}
          {sidebarVisible && <ServiceStatus compact={compact} />}
          {sidebarVisible && (
            <AskAiEntry
              compact={compact}
              onConnectTool={closeAfterNavigation(onConnectTool, closeOverlay)}
            />
          )}
          {sidebarVisible && onNavigateActivity && (
            <ActivityStatus
              compact={compact}
              current={activityCurrent}
              onOpenActivity={closeAfterNavigation(onNavigateActivity, closeOverlay)}
            />
          )}
          <ReviewEnvironmentBadge compact />
          <IdentityCard
            compact={compact}
            onOpenDetail={closeAfterNavigation(onEntityClick, closeOverlay)}
            onOpenSettings={closeAfterNavigation(onNavigateSettings, closeOverlay)}
            onOpenAbout={closeAfterNavigation(onOpenAbout, closeOverlay)}
          />
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
