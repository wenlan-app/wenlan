// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowCircleUp, GearSix, Info, Sparkle, User, WarningCircle } from "@phosphor-icons/react";
import { getProfile } from "../../lib/tauri";
import { useServiceNotices } from "../../lib/serviceNotices";
import { installUpdate, retryUpdate } from "../../lib/appUpdate";
import ProfileAvatar from "./ProfileAvatar";
import { AskAiDialog } from "./askAi/AskAiDialog";
import "./IdentityCard.css";

interface IdentityCardProps {
  compact?: boolean;
  onOpenDetail: (entityId: string) => void;
  onOpenSettings?: () => void;
  onOpenAbout?: () => void;
  /** Lets "What your AI can do" send the user to connect a tool. */
  onConnectTool?: () => void;
}

export default function IdentityCard({ compact = true, onOpenDetail, onOpenSettings, onOpenAbout, onConnectTool }: IdentityCardProps) {
  const { t } = useTranslation();
  const [menuOpen, setMenuOpen] = useState(false);
  const [keyboardNavigation, setKeyboardNavigation] = useState(false);
  // "What your AI can do" lives here rather than as its own footer row: new
  // users reach it from the getting-started checklist and the hints that show
  // when a command fits, and anyone can look it up again from this menu.
  const [askAiOpen, setAskAiOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const { data: profile } = useQuery({ queryKey: ["profile"], queryFn: getProfile });
  const displayName = profile?.display_name || profile?.name || "";
  const labelText = displayName || t("identityCard.account");
  // An app update never takes a row of its own in the sidebar footer. It is
  // all said on this entry: a calm dot while a version is on offer, a ring
  // that fills while it installs, an amber dot when it failed, and the one
  // action for each at the top of the menu.
  const { update } = useServiceNotices();
  const updateLabel = !update ? null
    : update.phase === "available" ? t("updater.available")
      : update.phase === "failed" ? t("updater.failed")
        : update.pct === null ? t("updater.updating")
          : t("updater.updatingPct", { pct: update.pct });
  const menuLabel = displayName
    ? t("identityCard.namedAccountMenu", { name: displayName })
    : t("identityCard.accountMenu");
  const triggerLabel = updateLabel ? `${menuLabel}, ${updateLabel}` : menuLabel;
  const glyph = displayName
    ? <ProfileAvatar avatarPath={profile?.avatar_path} displayName={displayName} size={compact ? 32 : 24} fontSize={compact ? 13 : 10} tone="muted" />
    : <User />;
  const glyphClass = displayName
    ? compact ? "identity-glyph" : "notes-navigation-glyph identity-account-slot identity-glyph"
    : "notes-navigation-glyph identity-glyph";

  useEffect(() => {
    if (!menuOpen) return;
    menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => document.removeEventListener("pointerdown", closeOutside);
  }, [menuOpen]);

  return (
    <div
      className="identity-rail-anchor"
      data-sidebar-escape-scope={menuOpen ? "true" : undefined}
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !menuOpen) return;
        event.preventDefault();
        event.stopPropagation();
        setMenuOpen(false);
        triggerRef.current?.focus();
      }}
      ref={rootRef}
    >
      <button
        aria-label={triggerLabel}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        className={`${compact ? "notes-rail-button" : "notes-primary-link"} identity-menu-trigger`}
        onClick={(event) => {
          // Virtual/keyboard activation has no pointer click count.
          setKeyboardNavigation(event.detail === 0);
          setMenuOpen((open) => !open);
        }}
        ref={triggerRef}
        title={labelText}
        type="button"
      >
        <span aria-hidden={displayName && compact ? undefined : "true"} className={glyphClass}>
          {glyph}
          {update?.phase === "available" && (
            <span className="identity-update-dot" data-testid="identity-update-dot" />
          )}
          {update?.phase === "failed" && (
            <span className="identity-update-dot identity-update-dot--failed" data-testid="identity-update-failed" />
          )}
          {update?.phase === "installing" && (
            <span
              className={`identity-update-ring${update.pct === null ? " identity-update-ring--waiting" : ""}`}
              data-testid="identity-update-ring"
              style={{ "--identity-update-pct": `${update.pct ?? 25}%` } as CSSProperties}
            />
          )}
        </span>
        {!compact && <span>{labelText}</span>}
      </button>
      {menuOpen && (
        <div
          aria-label={menuLabel}
          className="identity-rail-menu"
          data-keyboard-navigation={keyboardNavigation ? "true" : "false"}
          onKeyDown={(event) => {
            setKeyboardNavigation(true);
            if (event.key === "Tab") {
              setMenuOpen(false);
              return;
            }
            if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
            event.preventDefault();
            const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
            const current = items.indexOf(document.activeElement as HTMLButtonElement);
            const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
              : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
            items[next]?.focus();
          }}
          ref={menuRef}
          role="menu"
        >
          {update?.phase === "available" && (
            <button
              className="identity-menu-item identity-menu-item--update"
              data-testid="identity-update"
              onClick={() => {
                setMenuOpen(false);
                void installUpdate().catch(() => {});
              }}
              role="menuitem"
              title={t("updater.installBody")}
              type="button"
            >
              <span aria-hidden="true" className="notes-navigation-glyph"><ArrowCircleUp /></span>
              {t("updater.updateTo", { version: update.version })}
            </button>
          )}
          {update?.phase === "installing" && (
            <button
              className="identity-menu-item identity-menu-item--update"
              data-testid="identity-update-progress"
              disabled
              role="menuitem"
              title={t("updater.installBody")}
              type="button"
            >
              <span aria-hidden="true" className="notes-navigation-glyph"><ArrowCircleUp /></span>
              {updateLabel}
            </button>
          )}
          {update?.phase === "failed" && (
            <button
              className="identity-menu-item identity-menu-item--failed"
              data-testid="identity-update-retry"
              onClick={() => {
                setMenuOpen(false);
                void retryUpdate().catch(() => {});
              }}
              role="menuitem"
              title={t("updater.failedBody")}
              type="button"
            >
              <span aria-hidden="true" className="notes-navigation-glyph"><WarningCircle /></span>
              {t("updater.retry")}
            </button>
          )}
          <button
            className="identity-menu-item"
            data-testid="identity-ask-ai"
            onClick={() => { setMenuOpen(false); setAskAiOpen(true); }}
            role="menuitem"
            type="button"
          >
            <span aria-hidden="true" className="notes-navigation-glyph"><Sparkle /></span>
            {t("askAi.entry")}
          </button>
          <button
            className="identity-menu-item"
            onClick={() => {
              setMenuOpen(false);
              if (onOpenSettings) onOpenSettings();
              else onOpenDetail("__create_profile__");
            }}
            role="menuitem"
            type="button"
          >
            <span aria-hidden="true" className="notes-navigation-glyph"><GearSix /></span>
            {t("identityCard.settings")}
          </button>
          <button
            className="identity-menu-item"
            onClick={() => { setMenuOpen(false); onOpenAbout?.(); }}
            role="menuitem"
            type="button"
          >
            <span aria-hidden="true" className="notes-navigation-glyph"><Info /></span>
            {t("identityCard.aboutWenlan")}
          </button>
        </div>
      )}
      {askAiOpen ? (
        <AskAiDialog
          onClose={() => {
            setAskAiOpen(false);
            triggerRef.current?.focus();
          }}
          onConnectTool={onConnectTool && (() => {
            setAskAiOpen(false);
            onConnectTool();
          })}
        />
      ) : null}
    </div>
  );
}
