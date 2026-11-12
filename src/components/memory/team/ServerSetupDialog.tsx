// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Team › "Run it on a server": the steps for a team server on a
// machine nobody sits at, in the app rather than in a document. Type the
// address people reach it at and copy the command for the server. This
// Wenlan makes the owner's key and keeps it; the command carries only the
// key's SHA-256, so the key is never on the server's command line. Once
// the server is up this window joins it by itself: nobody reads the
// server's output or pastes anything back. From then on the owner runs the
// team from their own computer.
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { teamJoin } from "../../../lib/tauri";
import { TEAM_DOCS_URL } from "../sharing/team";
import "../sharing/sharing.css";
import "./server-setup.css";

export const SERVER_IMAGE = "ghcr.io/wenlan-app/wenlan-server";
const EXAMPLE_URL = "https://wenlan.example.com";
const EXAMPLE_HASH = "0".repeat(64);
/** How often the window asks whether the server is up yet. */
export const SERVER_POLL_MS = 5000;
// The owner's key this window made, kept until it has joined: closing the
// window before the server is up must not make a key the server never saw.
const OWNER_KEY_DRAFT = "wenlan-team-owner-key";

/** An address that can go in a shell command as is: http(s), no spaces or
 *  characters a shell would read. */
export function usableServerUrl(value: string): string | null {
  const url = value.trim().replace(/\/+$/, "");
  return /^https?:\/\/[A-Za-z0-9.\-[\]:]+(\/[A-Za-z0-9._~\-/]*)?$/.test(url) ? url : null;
}

/** A new owner key, the shape the server takes: `wlt_` and 64 hex digits. */
export function newOwnerKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return `wlt_${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

function draftOwnerKey(): string {
  try {
    const kept = localStorage.getItem(OWNER_KEY_DRAFT);
    if (kept && /^wlt_[0-9a-f]{64}$/.test(kept)) return kept;
    const made = newOwnerKey();
    localStorage.setItem(OWNER_KEY_DRAFT, made);
    return made;
  } catch {
    return newOwnerKey();
  }
}

/** Forgets the owner key once this computer has joined with it. */
export function forgetOwnerKeyDraft(): void {
  try { localStorage.removeItem(OWNER_KEY_DRAFT); } catch { /* nothing kept */ }
}

/** The owner's invite link: what this window joins with. */
export function ownerLink(url: string, key: string): string {
  return `${url}/join#${key}`;
}

/** The SHA-256 the server keeps of a key, as lowercase hex. */
export async function keySha256(key: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(key.trim()));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** The one line run on the server: the address, and the owner's key by its
 *  SHA-256 only. Nothing needs reading from it after: this window joins
 *  by itself. */
export function serverCommand(url: string, ownerHash: string): string {
  return `docker run -d --name wenlan --restart unless-stopped -p 7879:7879 -v wenlan-data:/data ${SERVER_IMAGE} team "${url}" --owner ${ownerHash}`;
}

export function ServerSetupDialog({ onClose, onPaste, onReady }: {
  readonly onClose: () => void;
  readonly onPaste: () => void;
  /** The server answered with this owner's link: join it. */
  readonly onReady: (link: string) => void;
}) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [address, setAddress] = useState("");
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);
  const [key] = useState(draftOwnerKey);
  const [hash, setHash] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    void keySha256(key).then((made) => { if (current) setHash(made); });
    return () => { current = false; };
  }, [key]);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  const url = usableServerUrl(address);
  const ready = Boolean(url && hash);
  const link = url ? ownerLink(url, key) : null;
  const command = serverCommand(url ?? EXAMPLE_URL, (url && hash) || EXAMPLE_HASH);
  // Once the command is copied, ask the server every few seconds whether
  // it is up and knows this key; the first answer opens the join window.
  const joined = useRef(onReady);
  joined.current = onReady;
  useEffect(() => {
    if (!copied || !link) return undefined;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ask = async () => {
      try {
        await teamJoin(link);
        if (!stopped) joined.current(link);
        return;
      } catch {
        // Not up yet, or not reachable from here yet: ask again.
      }
      if (!stopped) timer = setTimeout(() => void ask(), SERVER_POLL_MS);
    };
    timer = setTimeout(() => void ask(), SERVER_POLL_MS);
    return () => { stopped = true; clearTimeout(timer); };
  }, [copied, link]);
  const copy = async () => {
    if (!ready) return;
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
            <button type="button" className="share-secondary" onClick={() => void copy()} disabled={!ready}>
              {t(copied ? "team.copied" : "team.copy")}
            </button>
          </div>
          <pre className={`server-setup-command${ready ? "" : " server-setup-command-example"}`}>
            {command.split("\n").map((line) => <code key={line}>{line}</code>)}
          </pre>
          <span className="share-hint">{t("team.server.runHint")}</span>
          {failed && <span className="share-warning" role="alert">{t("team.server.copyFailed")}</span>}
        </li>
        <li>
          <span className="share-label">{t("team.server.paste")}</span>
          <span className="share-hint">{t("team.server.pasteHint")}</span>
          {copied && <span className="server-setup-waiting" role="status">{t("team.server.waiting")}</span>}
          <span><button type="button" className="share-link" onClick={onPaste}>{t("team.server.pasteAction")}</button></span>
        </li>
      </ol>
      <div className="share-actions server-setup-actions">
        <button type="button" className="share-link" onClick={openDocs}>{t("team.server.more")}</button>
        <button type="button" className="share-secondary" onClick={onClose}>{t("team.server.done")}</button>
      </div>
    </dialog>
  );
}
