// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Team › "Run it on a server": the steps for a team server on a
// machine nobody sits at, in the app rather than in a document. Type the
// address people reach it at, copy the command for the server, then paste
// the owner's link it prints. From then on the owner runs the team from
// their own computer.
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { TEAM_DOCS_URL } from "../sharing/team";
import "../sharing/sharing.css";
import "./server-setup.css";

export const SERVER_IMAGE = "ghcr.io/wenlan-app/wenlan-server";
const EXAMPLE_URL = "https://wenlan.example.com";

/** An address that can go in a shell command as is: http(s), no spaces or
 *  characters a shell would read. */
export function usableServerUrl(value: string): string | null {
  const url = value.trim().replace(/\/+$/, "");
  return /^https?:\/\/[A-Za-z0-9.\-[\]:]+(\/[A-Za-z0-9._~\-/]*)?$/.test(url) ? url : null;
}

/** The two lines run on the server: start it, then show the owner's link. */
export function serverCommand(url: string): string {
  return [
    `docker run -d --name wenlan --restart unless-stopped -p 7879:7879 -v wenlan-data:/data ${SERVER_IMAGE} team ${url}`,
    "docker logs wenlan",
  ].join("\n");
}

export function ServerSetupDialog({ onClose, onPaste }: {
  readonly onClose: () => void;
  readonly onPaste: () => void;
}) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [address, setAddress] = useState("");
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  const url = usableServerUrl(address);
  const command = serverCommand(url ?? EXAMPLE_URL);
  const copy = async () => {
    if (!url) return;
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      setFailed(false);
    } catch {
      setFailed(true);
    }
  };
  const openDocs = () => { void Promise.resolve(openExternal(TEAM_DOCS_URL)).catch(() => undefined); };

  return (
    <dialog
      ref={dialogRef}
      className="share-dialog server-setup"
      aria-labelledby="server-setup-title"
      onCancel={(event) => { event.preventDefault(); onClose(); }}
    >
      <h2 id="server-setup-title">{t("team.server.title")}</h2>
      <p className="share-hint">{t("team.server.intro")}</p>
      <ol className="server-setup-steps">
        <li>
          <label className="share-field">
            <span className="share-label">{t("team.server.address")}</span>
            <input
              className="share-input"
              value={address}
              placeholder={t("team.server.addressPlaceholder")}
              spellCheck={false}
              autoFocus
              onChange={(event) => { setAddress(event.target.value); setCopied(false); }}
            />
            <span className="share-hint">{t("team.server.addressHint")}</span>
          </label>
        </li>
        <li>
          <div className="server-setup-run-head">
            <span className="share-label">{t("team.server.run")}</span>
            <button type="button" className="share-secondary" onClick={() => void copy()} disabled={!url}>
              {t(copied ? "team.copied" : "team.copy")}
            </button>
          </div>
          <pre className={`server-setup-command${url ? "" : " server-setup-command-example"}`}>
            {command.split("\n").map((line) => <code key={line}>{line}</code>)}
          </pre>
          <span className="share-hint">{t("team.server.runHint")}</span>
          {failed && <span className="share-warning" role="alert">{t("team.server.copyFailed")}</span>}
        </li>
        <li>
          <div className="server-setup-run-head">
            <span className="share-label">{t("team.server.paste")}</span>
            <button type="button" className="share-primary" onClick={onPaste}>{t("team.server.pasteAction")}</button>
          </div>
          <span className="share-hint">{t("team.server.pasteHint")}</span>
        </li>
      </ol>
      <div className="share-actions server-setup-actions">
        <button type="button" className="share-link" onClick={openDocs}>{t("team.server.more")}</button>
        <button type="button" className="share-secondary" onClick={onClose}>{t("team.server.done")}</button>
      </div>
    </dialog>
  );
}
