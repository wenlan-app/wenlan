// SPDX-License-Identifier: AGPL-3.0-only
// The member's own dialogs, opened from their account menu: the AI tools
// they connected with `wenlan login` (each removable), and a one-time link
// that opens the team in the desktop app.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ProviderMark } from "./ProviderMarks";
import { desktopLink, reauthThen, removeTeamTool, teamTools, type TeamTool } from "./team";
import "../components/memory/sharing/sharing.css";
import "./teamWeb.css";

function Dialog({ label, testId, onClose, children }: {
  readonly label: string;
  readonly testId: string;
  readonly onClose: () => void;
  readonly children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    if (dialog && !dialog.open) dialog.showModal?.();
    return () => dialog?.close?.();
  }, []);
  return (
    <dialog
      ref={ref}
      className="share-dialog team-own-dialog"
      aria-label={label}
      data-testid={testId}
      onCancel={(event) => { event.preventDefault(); onClose(); }}
    >
      {children}
    </dialog>
  );
}

/** `2026-11-21T…` as the reader's own short date. */
export function shortDate(at: string, language: string): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return at.slice(0, 10);
  return date.toLocaleDateString(language, { month: "short", day: "numeric" });
}

export function ToolsDialog({ team, onChanged, onClose }: {
  readonly team: string;
  /** How many tools are left, after a change. */
  readonly onChanged: (count: number) => void;
  readonly onClose: () => void;
}) {
  const { t, i18n } = useTranslation();
  const [tools, setTools] = useState<TeamTool[] | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let live = true;
    void teamTools().then((found) => {
      if (!live) return;
      if (found) setTools(found);
      else setFailed(true);
    });
    return () => { live = false; };
  }, []);
  const remove = async (tool: TeamTool) => {
    setFailed(false);
    const left = await removeTeamTool(tool.id);
    if (!left) {
      setFailed(true);
      return;
    }
    setTools(left);
    onChanged(left.length);
  };
  return (
    <Dialog label={t("teamWeb.toolsTitle")} testId="team-tools-dialog" onClose={onClose}>
      <h2>{t("teamWeb.toolsTitle")}</h2>
      <p>{t("teamWeb.toolsHint", { team })}</p>
      {tools && tools.length > 0 && (
        <ul className="team-own-list">
          {tools.map((tool) => (
            <li key={tool.id} data-testid="team-tool-row">
              <span>{tool.name}</span>
              <span className="team-own-when">{t("teamWeb.toolsAdded", { date: shortDate(tool.created_at, i18n.language) })}</span>
              <button type="button" aria-label={t("teamWeb.removeName", { name: tool.name })} onClick={() => void remove(tool)}>{t("teamWeb.remove")}</button>
            </li>
          ))}
        </ul>
      )}
      {tools && tools.length === 0 && <p data-testid="team-tools-none">{t("teamWeb.toolsNone")}</p>}
      <p>{t("teamWeb.toolsAdd")}</p>
      <div className="team-own-link">
        <code data-testid="team-tools-command">wenlan login {window.location.origin}</code>
      </div>
      {failed && <p className="team-account-error" role="alert">{t("teamWeb.toolsFailed")}</p>}
      <div className="team-own-foot">
        <button type="button" className="team-device-primary" onClick={onClose}>{t("teamWeb.done")}</button>
      </div>
    </Dialog>
  );
}

/** The link with its key hidden: `…/join#wlt_••••••••••••`. */
function masked(link: string): string {
  const at = link.indexOf("#wlt_");
  return at < 0 ? link : `${link.slice(0, at + 5)}${"•".repeat(12)}`;
}

function providerName(provider: string): string {
  return provider === "github" ? "GitHub" : "Google";
}

type DesktopState =
  | { kind: "making" }
  | { kind: "link"; link: string }
  | { kind: "ask" }
  | { kind: "failed" };

export function DesktopDialog({ team, provider, onClose }: {
  readonly team: string;
  /** The account linked, which confirms it's them. */
  readonly provider: string;
  readonly onClose: () => void;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<DesktopState>({ kind: "making" });
  const [copied, setCopied] = useState(false);
  const asked = useRef(false);
  useEffect(() => {
    if (asked.current) return;
    asked.current = true;
    void desktopLink().then((made) => {
      if (made.ok) setState({ kind: "link", link: made.value.link });
      else setState(made.error === "reauth_required" ? { kind: "ask" } : { kind: "failed" });
    });
  }, []);
  const copy = async (link: string) => {
    try {
      await navigator.clipboard.writeText(link);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };
  const confirm = async () => {
    if (!(await reauthThen(provider, { kind: "desktop" }))) setState({ kind: "failed" });
  };
  return (
    <Dialog label={t("teamWeb.desktopTitle", { team })} testId="team-desktop-dialog" onClose={onClose}>
      <h2>{t("teamWeb.desktopTitle", { team })}</h2>
      {state.kind === "ask" && (
        <>
          <p>{t("teamWeb.desktopAsk", { provider: providerName(provider) })}</p>
          <button type="button" className="team-signin-button" data-testid="team-desktop-confirm" onClick={() => void confirm()}>
            <ProviderMark provider={provider} />
            {provider === "github" ? t("teamWeb.continueGitHub") : t("teamWeb.continueGoogle")}
          </button>
        </>
      )}
      {state.kind === "link" && (
        <>
          <p>{t("teamWeb.desktopStep1")}</p>
          <div className="team-own-link">
            <code data-testid="team-desktop-link">{masked(state.link)}</code>
            <button type="button" className="team-device-primary" onClick={() => void copy(state.link)}>
              {copied ? t("teamWeb.copied") : t("teamWeb.copy")}
            </button>
          </div>
          <p>{t("teamWeb.desktopStep2")}</p>
          <p className="team-device-quiet">
            <ProviderMark provider={provider} size={13} /> {t("teamWeb.desktopConfirmed", { provider: providerName(provider) })}
          </p>
        </>
      )}
      {state.kind === "failed" && <p className="team-account-error" role="alert">{t("teamWeb.desktopFailed")}</p>}
      <div className="team-own-foot">
        <button type="button" className="team-device-primary" onClick={onClose}>{t("teamWeb.done")}</button>
      </div>
    </Dialog>
  );
}
