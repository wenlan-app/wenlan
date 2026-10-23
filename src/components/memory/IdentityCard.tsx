// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowCircleUp, GearSix, Info, User } from "@phosphor-icons/react";
import { getProfile } from "../../lib/tauri";
import { useServiceNotices } from "../../lib/serviceNotices";
import { installUpdate } from "../../lib/appUpdate";
import ProfileAvatar from "./ProfileAvatar";
import "./IdentityCard.css";

interface IdentityCardProps {
  compact?: boolean;
  onOpenDetail: (entityId: string) => void;
  onOpenSettings?: () => void;
  onOpenAbout?: () => void;
}

export default function IdentityCard({ compact = true, onOpenDetail, onOpenSettings, onOpenAbout }: IdentityCardProps) {
  const { t } = useTranslation();
  const [menuOpen, setMenuOpen] = useState(false);
  const [keyboardNavigation, setKeyboardNavigation] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const { data: profile } = useQuery({ queryKey: ["profile"], queryFn: getProfile });
  const displayName = profile?.display_name || profile?.name || "";
  const labelText = displayName || t("identityCard.account");
  // A new version on offer is not urgent: a dot here and one item in the
  // menu, instead of a row of its own in the sidebar footer. Once installing
  // or failed it moves to the service row (src/components/memory/ServiceStatus.tsx).
  const { update } = useServiceNotices();
  const offer = update?.phase === "available" ? update : null;
  const menuLabel = displayName
    ? t("identityCard.namedAccountMenu", { name: displayName })
    : t("identityCard.accountMenu");
  const triggerLabel = offer ? `${menuLabel}, ${t("updater.available")}` : menuLabel;

  useEffect(() => {
    if (!menuOpen) return;
    menuRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
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
        {displayName ? compact ? (
          <ProfileAvatar avatarPath={profile?.avatar_path} displayName={displayName} size={32} fontSize={13} tone="muted" />
        ) : (
          <span aria-hidden="true" className="notes-navigation-glyph identity-account-slot">
            <ProfileAvatar avatarPath={profile?.avatar_path} displayName={displayName} size={24} fontSize={10} tone="muted" />
          </span>
        ) : (
          <span aria-hidden="true" className="notes-navigation-glyph"><User /></span>
        )}
        {!compact && <span>{labelText}</span>}
        {offer && <span aria-hidden="true" className="identity-update-dot" data-testid="identity-update-dot" />}
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
            const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? []);
            const current = items.indexOf(document.activeElement as HTMLButtonElement);
            const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
              : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
            items[next]?.focus();
          }}
          ref={menuRef}
          role="menu"
        >
          {offer && (
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
              {t("updater.updateTo", { version: offer.version })}
            </button>
          )}
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
    </div>
  );
}
