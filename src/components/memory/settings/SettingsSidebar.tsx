// SPDX-License-Identifier: AGPL-3.0-only
//
// Settings sidebar. Replaces the default Spaces/Profile sidebar when the
// Settings page is active. It keeps the same width, background, border, and
// footer brand treatment as the main Sidebar so the transition feels like
// "the sidebar switched modes" rather than "a different layout loaded".

import { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ArrowSquareOut, ShieldCheck } from "@phosphor-icons/react";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { ICONS_SIDEBAR_WIDTH, type SidebarMode } from "../navigation/navigationPreferences";

export type SettingsSection =
  | "sources"
  | "agents"
  | "intelligence"
  | "diagnostics"
  | "general"
  | "appearance"
  | "team"
  | "experiments";

type SettingsGroupLabelKey =
  | "settings.groups.general.label"
  | "settings.groups.appearance.label"
  | "settings.groups.intelligence.label"
  | "settings.groups.diagnostics.label"
  | "settings.groups.agents.label"
  | "settings.groups.sources.label"
  | "settings.groups.team.label"
  | "settings.groups.experiments.label";

type SettingsGroupHintKey =
  | "settings.groups.general.hint"
  | "settings.groups.appearance.hint"
  | "settings.groups.intelligence.hint"
  | "settings.groups.diagnostics.hint"
  | "settings.groups.agents.hint"
  | "settings.groups.sources.hint"
  | "settings.groups.team.hint"
  | "settings.groups.experiments.hint";

export interface SettingsGroup {
  id: SettingsSection;
  labelKey: SettingsGroupLabelKey;
  /** One-line caption shown beneath the label when the group is active. */
  hintKey: SettingsGroupHintKey;
  icon: React.ReactNode;
}

/**
 * The master list of settings groups. Two places it's consumed:
 *   1. The sidebar rendering (below) — icon + label per button.
 *   2. `SettingsPage.tsx` gating — each `<section>` checks `section === "..."`.
 *
 * Ordering is the display order in the sidebar. Groups that used to be
 * separate sections (Sources + Import; Connected Agents + Remote Access)
 * are consolidated into one group each — they were related enough that
 * stacking them as siblings was just noise.
 */
export const SETTINGS_GROUPS: SettingsGroup[] = [
  {
    id: "general",
    labelKey: "settings.groups.general.label",
    hintKey: "settings.groups.general.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
      </svg>
    ),
  },
  {
    id: "appearance",
    labelKey: "settings.groups.appearance.label",
    hintKey: "settings.groups.appearance.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <path d="M12 3a9 9 0 100 18c1.1 0 1.7-.9 1.7-1.8 0-.5-.2-.9-.5-1.2-.3-.3-.5-.7-.5-1.2 0-1 .8-1.8 1.8-1.8H17a4 4 0 004-4c0-4.4-4-8-9-8z" />
        <circle cx="7.5" cy="11.5" r="1" />
        <circle cx="10.5" cy="7.5" r="1" />
        <circle cx="15.5" cy="7.5" r="1" />
      </svg>
    ),
  },
  {
    id: "intelligence",
    labelKey: "settings.groups.intelligence.label",
    hintKey: "settings.groups.intelligence.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="12" cy="12" r="9" />
        <path d="M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6L5.6 18.4" />
      </svg>
    ),
  },
  {
    id: "diagnostics",
    labelKey: "settings.groups.diagnostics.label",
    hintKey: "settings.groups.diagnostics.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <path d="M4 19V5" />
        <path d="M4 19h16" />
        <path d="M8 15l3-3 3 2 4-6" />
      </svg>
    ),
  },
  {
    id: "agents",
    labelKey: "settings.groups.agents.label",
    hintKey: "settings.groups.agents.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <path d="M13.828 10.172a4 4 0 00-5.656 0l-4 4a4 4 0 105.656 5.656l1.102-1.101m-.758-4.899a4 4 0 005.656 0l4-4a4 4 0 00-5.656-5.656l-1.1 1.1" />
      </svg>
    ),
  },
  {
    id: "sources",
    labelKey: "settings.groups.sources.label",
    hintKey: "settings.groups.sources.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <ellipse cx="12" cy="5" rx="9" ry="3" />
        <path d="M3 5v14c0 1.7 4 3 9 3s9-1.3 9-3V5" />
        <path d="M3 12c0 1.7 4 3 9 3s9-1.3 9-3" />
      </svg>
    ),
  },
  {
    id: "team",
    labelKey: "settings.groups.team.label",
    hintKey: "settings.groups.team.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="9" cy="8" r="3.2" />
        <path d="M3 20a6 6 0 0112 0" />
        <path d="M16 5.2a3.2 3.2 0 010 5.6M18 14.4A6 6 0 0121 20" />
      </svg>
    ),
  },
  {
    id: "experiments",
    labelKey: "settings.groups.experiments.label",
    hintKey: "settings.groups.experiments.hint",
    icon: (
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <path d="M9 3h6" />
        <path d="M10 3v6.5L4.8 18.3A1.8 1.8 0 006.4 21h11.2a1.8 1.8 0 001.6-2.7L14 9.5V3" />
        <path d="M7.5 14.5h9" />
      </svg>
    ),
  },
];

interface SettingsSidebarProps {
  hidden: boolean;
  mode: SidebarMode;
  active: SettingsSection;
  onSelect: (section: SettingsSection) => void;
  open?: boolean;
  presentation?: "desktop" | "overlay";
  onRequestClose?: () => void;
  onNavigateHome?: () => void;
  navigationDisabled?: boolean;
}

export default function SettingsSidebar({
  hidden,
  mode,
  active,
  onSelect,
  open = !hidden,
  presentation = "desktop",
  onRequestClose,
  onNavigateHome,
  navigationDisabled = false,
}: SettingsSidebarProps) {
  const { t } = useTranslation();
  const [appVersion, setAppVersion] = useState<string>("");
  const [linkFailed, setLinkFailed] = useState(false);
  const asideRef = useRef<HTMLElement>(null);
  const overlay = presentation === "overlay";
  const compact = !overlay && mode === "icons";
  const sidebarVisible = overlay ? open : !hidden;
  const contentInert = !sidebarVisible;

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => setAppVersion(""));
  }, []);
  useEffect(() => {
    if (!overlay || !open) return;
    const first = asideRef.current?.querySelector<HTMLElement>(
      "button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])",
    );
    first?.focus();
  }, [open, overlay]);

  const trapFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== "Tab" || !overlay) return;
    const focusable = asideRef.current?.querySelectorAll<HTMLElement>(
      "button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])",
    );
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
        aria-label={t("settings.title")}
        className={`settings-sidebar flex-shrink-0 flex flex-col${compact ? " settings-sidebar--icons" : ""}`}
        data-sidebar-overlay={overlay && open ? "true" : undefined}
        inert={contentInert}
        onKeyDown={trapFocus}
        ref={asideRef}
        style={{
          width: overlay ? 240 : hidden ? 0 : compact ? ICONS_SIDEBAR_WIDTH : "var(--workspace-sidebar-width, 240px)",
          backgroundColor: "var(--workspace-sidebar-background, var(--mem-sidebar))",
          borderRight: "none",
          bottom: overlay ? 0 : undefined,
          height: overlay ? "auto" : "100%",
          left: overlay ? 0 : undefined,
          overflow: "hidden",
          position: overlay ? "fixed" : "relative",
          top: overlay ? 52 : undefined,
          transform: overlay && !open ? "translateX(-100%)" : undefined,
          visibility: overlay && !open ? "hidden" : "visible",
          zIndex: overlay ? 40 : 2,
        }}
      >
      <div
        className="flex flex-col h-full transition-opacity duration-150"
        style={{
          width: overlay ? 240 : compact ? ICONS_SIDEBAR_WIDTH : "var(--workspace-sidebar-width, 240px)",
          opacity: contentInert ? 0 : 1,
          pointerEvents: contentInert ? "none" : "auto",
        }}
      >
        <div className={`notes-sidebar-search${compact ? " notes-sidebar-search--compact" : ""}`}>
          <button
            aria-label={t("settings.home")}
            className={`notes-search-entry${compact ? " notes-search-entry--compact" : ""}`}
            disabled={navigationDisabled || !onNavigateHome}
            onClick={onNavigateHome}
            title={compact ? t("settings.home") : undefined}
            type="button"
          >
            <ArrowLeft aria-hidden="true" size={22} weight="regular" />
            {!compact && <span className="notes-search-entry-label">{t("settings.home")}</span>}
          </button>
        </div>

        {/* Group list */}
        <div className="notes-icon-rail">
          <nav aria-label={t("settings.title")} className={`notes-rail-nav${compact ? " notes-rail-nav--icons" : " notes-rail-nav--labels"}`}>
            {SETTINGS_GROUPS.map((group) => {
              const isActive = group.id === active;
              return (
                <button
                  key={group.id}
                  aria-label={t(group.labelKey)}
                  title={compact ? t(group.labelKey) : undefined}
                  onClick={() => onSelect(group.id)}
                  className="notes-primary-link settings-sidebar-group"
                  data-active={isActive ? "true" : undefined}
                  type="button"
                >
                  <span className="notes-navigation-glyph settings-sidebar-group-icon">
                    {group.icon}
                  </span>
                  <span className="settings-sidebar-group-label">
                    {t(group.labelKey)}
                  </span>
                </button>
              );
            })}
          </nav>
        </div>

        <div className="settings-sidebar-policy">
          {linkFailed && <div className="settings-sidebar-link-error" role="alert">{t("aboutWenlan.linkError")}</div>}
          <a
            aria-label={t("settings.privacyPolicyLabel")}
            className={`settings-sidebar-privacy${compact ? " settings-sidebar-privacy--compact" : ""}`}
            href="https://github.com/wenlan-app/wenlan/blob/main/docs/PRIVACY.md"
            onClick={(event) => {
              event.preventDefault();
              setLinkFailed(false);
              void openExternal(event.currentTarget.href).catch(() => setLinkFailed(true));
            }}
            title={compact ? t("settings.privacyPolicyLabel") : undefined}
          >
            {compact ? <ShieldCheck aria-hidden="true" size={22} weight="regular" /> : <span>{t("settings.privacyPolicyLabel")}</span>}
            {!compact && <ArrowSquareOut aria-hidden="true" className="settings-sidebar-privacy-external" size={13} weight="regular" />}
          </a>
        </div>
        {/* Bottom brand strip — pinned. Mirrors main Sidebar pattern. */}
        <div className="settings-sidebar-footer px-4 pt-3 pb-4 flex-shrink-0">
          <div className="flex items-baseline justify-between gap-2">
            <span className="settings-sidebar-brand"
              style={{
                color: "var(--mem-text-tertiary)",
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-xs)",
                fontWeight: 600,
                lineHeight: 1,
              }}
            >
              Wenlan
            </span>
            {appVersion && (
              <span className="settings-sidebar-version"
                style={{
                  fontFamily: "var(--mem-font-mono)",
                  fontSize: "var(--mem-text-2xs)",
                  color: "var(--mem-text-tertiary)",
                  letterSpacing: "0.02em",
                  opacity: 0.7,
                  whiteSpace: "nowrap",
                }}
              >
                v{appVersion}
              </span>
            )}
          </div>
        </div>
      </div>
      </aside>
    </>
  );
}
