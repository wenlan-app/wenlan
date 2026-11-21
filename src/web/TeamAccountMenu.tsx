// SPDX-License-Identifier: AGPL-3.0-only
// The bottom of the web sidebar for a team member: their avatar letter and
// name, and a menu above it. Signed in with a browser session (a Google or
// GitHub account), it signs out, or out everywhere. Joined with an invite
// link (a key kept in this browser), it offers adding Google or GitHub so
// they can sign in from any device, and signs out of this browser.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Desktop, Plug, SignOut } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { confirmAction } from "../lib/confirmAction";
import { ProviderMark } from "./ProviderMarks";
import { DesktopDialog, ToolsDialog } from "./TeamOwnDialogs";
import {
  forgetTeamKey,
  linkProvider,
  peekAfter,
  signOutBrowser,
  signOutEverywhere,
  takeAfter,
  teamMe,
  teamProviders,
  teamTools,
  type TeamMe,
} from "./team";
import "./teamWeb.css";

const KNOWN = ["google", "github"];

function initialOf(name: string): string {
  return [...name.trim()][0]?.toUpperCase() ?? "?";
}

export function TeamAccountMenu({ compact = false, className, labelClassName, glyphClassName }: {
  readonly compact?: boolean;
  /** The row's own class, so it looks like its neighbors. */
  readonly className: string;
  readonly labelClassName?: string;
  readonly glyphClassName?: string;
}) {
  const { t } = useTranslation();
  const [me, setMe] = useState<TeamMe | null>(null);
  const [providers, setProviders] = useState<string[]>([]);
  const [open, setOpen] = useState(false);
  const [failed, setFailed] = useState(false);
  const [place, setPlace] = useState<{ left: number; bottom: number } | null>(null);
  const [tools, setTools] = useState<number | null>(null);
  const [dialog, setDialog] = useState<"tools" | "desktop" | null>(null);
  const rowRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    void teamMe().then((found) => {
      if (!live) return;
      setMe(found);
      // Back from confirming it's them, to make the desktop link.
      if (found?.signed_in_with === "browser" && peekAfter()?.kind === "desktop") {
        takeAfter();
        setDialog("desktop");
      }
    });
    return () => { live = false; };
  }, []);
  useEffect(() => {
    if (!me || !open) return;
    let live = true;
    void teamTools().then((found) => { if (live && found) setTools(found.length); });
    return () => { live = false; };
  }, [me, open]);
  const key = me?.signed_in_with === "key";
  useEffect(() => {
    if (!key) return;
    let live = true;
    void teamProviders().then((found) => { if (live) setProviders(found.providers); });
    return () => { live = false; };
  }, [key]);

  useLayoutEffect(() => {
    if (!open || !rowRef.current) return;
    const rect = rowRef.current.getBoundingClientRect();
    setPlace({ left: Math.max(14, rect.left + 8), bottom: Math.max(8, window.innerHeight - rect.top + 6) });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const away = (event: MouseEvent) => {
      const target = event.target as Node;
      if (menuRef.current?.contains(target) || rowRef.current?.contains(target)) return;
      setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setOpen(false);
      rowRef.current?.focus();
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", escape, true);
    };
  }, [open]);

  if (!me) return null;
  const account = me.accounts?.[0];
  const team = me.team?.name ?? t("teamWeb.teamFallback");
  const pick = (next: "tools" | "desktop") => {
    setOpen(false);
    setDialog(next);
  };
  const toolsItem = (
    <button type="button" role="menuitem" className="team-account-item" data-testid="team-account-tools" onClick={() => pick("tools")}>
      <Plug size={18} aria-hidden="true" />
      <span className="team-account-item-text">{t("teamWeb.toolsItem")}</span>
      {tools !== null && tools > 0 && <span className="team-account-count">{tools}</span>}
    </button>
  );

  const everywhere = async () => {
    setOpen(false);
    const ok = await confirmAction(t("teamWeb.everywhereConfirm"), {
      confirm: t("teamWeb.everywhere"),
      cancel: t("sharing.cancel"),
      tone: "neutral",
    });
    if (ok) await signOutEverywhere();
  };
  const link = async (provider: string) => {
    setFailed(false);
    if (!(await linkProvider(provider))) setFailed(true);
  };

  return (
    <>
      <button
        ref={rowRef}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t("teamWeb.menuFor", { name: me.name })}
        className={`${className} team-account-row`}
        data-testid="team-account-row"
        onClick={() => setOpen(!open)}
        title={compact ? me.name : undefined}
      >
        <span aria-hidden="true" className={`${glyphClassName ?? ""} team-account-face`}>{initialOf(me.name)}</span>
        {!compact && <span className={`${labelClassName ?? ""} team-account-name`}>{me.name}</span>}
      </button>
      {open && createPortal(
        <div
          ref={menuRef}
          className="team-account-menu"
          data-testid="team-account-menu"
          role="menu"
          style={place ? { left: place.left, bottom: place.bottom } : { visibility: "hidden" }}
        >
          <div className="team-account-head">
            <span className="team-account-big" aria-hidden="true">{initialOf(me.name)}</span>
            <div className="team-account-who">
              <b>{me.name}</b>
              {key ? (
                <span>{t("teamWeb.joinedWithLink")}</span>
              ) : account ? (
                <span data-testid="team-account-email">
                  <ProviderMark provider={account.provider} size={13} />
                  {account.email && <span>{account.email}</span>}
                </span>
              ) : null}
            </div>
          </div>
          {key ? (
            <>
              {providers.length > 0 && (
                <div className="team-account-save" data-testid="team-account-link-card">
                  <b>{t("teamWeb.anyDeviceTitle")}</b>
                  <p>{t("teamWeb.anyDeviceBody")}</p>
                  {providers.filter((provider) => KNOWN.includes(provider)).map((provider) => (
                    <button key={provider} type="button" className="team-signin-button" data-testid={`team-account-link-${provider}`} onClick={() => void link(provider)}>
                      <ProviderMark provider={provider} />
                      {provider === "google" ? t("teamWeb.continueGoogle") : t("teamWeb.continueGitHub")}
                    </button>
                  ))}
                  {failed && <p className="team-account-error" role="alert">{t("teamWeb.linkFailed")}</p>}
                </div>
              )}
              {toolsItem}
              <button type="button" role="menuitem" className="team-account-item" data-testid="team-web-forget" onClick={forgetTeamKey}>
                <SignOut size={18} aria-hidden="true" />
                <span className="team-account-item-text">
                  <span>{t("teamWeb.forget")}</span>
                  <span className="team-account-item-sub">{t("teamWeb.forgetHint")}</span>
                </span>
              </button>
            </>
          ) : (
            <>
              <div className="team-account-separator" role="separator" />
              {toolsItem}
              {account && (
                <button type="button" role="menuitem" className="team-account-item" data-testid="team-account-desktop" onClick={() => pick("desktop")}>
                  <Desktop size={18} aria-hidden="true" />
                  <span className="team-account-item-text">{t("teamWeb.desktopItem")}</span>
                </button>
              )}
              <div className="team-account-separator" role="separator" />
              <button type="button" role="menuitem" className="team-account-item" data-testid="team-account-signout" onClick={() => void signOutBrowser()}>
                <SignOut size={18} aria-hidden="true" />
                <span>{t("teamWeb.signOut")}</span>
              </button>
              <div className="team-account-separator" role="separator" />
              <p className="team-account-quiet">
                {t("teamWeb.everywhereAsk")}{" "}
                <button type="button" data-testid="team-account-everywhere" onClick={() => void everywhere()}>{t("teamWeb.everywhere")}</button>
              </p>
            </>
          )}
        </div>,
        document.body,
      )}
      {dialog === "tools" && <ToolsDialog team={team} onChanged={setTools} onClose={() => setDialog(null)} />}
      {dialog === "desktop" && account && <DesktopDialog team={team} provider={account.provider} onClose={() => setDialog(null)} />}
    </>
  );
}
