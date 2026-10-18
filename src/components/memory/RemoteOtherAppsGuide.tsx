// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy } from "@phosphor-icons/react";
import { clipboardWrite } from "../../lib/tauri";
import { Button } from "./settings/primitives";
import "./pairingApproval.css";
import "./remoteOtherAppsGuide.css";

/**
 * A short first-time guide for AI apps Wenlan has no button for: three steps
 * and the link to paste, already copied when it opens. It uses the approval dialog's modal shell (backdrop,
 * focus trap, Escape), and hands focus back to the button that opened it.
 */
export function RemoteOtherAppsGuide({ relayUrl, returnFocus, onClose }: {
  relayUrl: string;
  /** The control that opened the guide; it gets focus back when the guide closes. */
  returnFocus: HTMLElement | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const panel = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    panel.current?.focus();
    // Some webviews do not focus a button on click, so the opener is named, not sniffed.
    return () => { if (returnFocus?.isConnected) returnFocus.focus(); };
  }, [returnFocus]);

  const copy = async () => {
    // A failed copy leaves the link on screen to select; nothing claims it was copied.
    try { await clipboardWrite(relayUrl); setCopied(true); } catch { setCopied(false); }
  };

  // Opening the guide copies the link, the same as Add to ChatGPT does.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { void copy(); }, []);

  return (
    <div className="pairing-approval-backdrop">
      <div ref={panel} tabIndex={-1} role="dialog" aria-modal="true" aria-labelledby={titleId}
        className="pairing-approval-panel remote-guide-panel"
        onKeyDown={(event) => {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(); return; }
          if (event.key !== "Tab") return;
          const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>("button:not(:disabled), a[href]") ?? []);
          const first = controls[0];
          const last = controls[controls.length - 1];
          if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) {
            event.preventDefault(); last?.focus();
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault(); first?.focus();
          }
        }}>
        <h2 id={titleId}>{t("remoteAccess.otherAppsTitle")}</h2>
        <ol className="remote-guide-steps">
          <li>{t("remoteAccess.otherAppsStep1")}</li>
          <li>
            <span>{t("remoteAccess.otherAppsStep2")}</span>
            <span className="remote-guide-link">
              <code>{relayUrl}</code>
              <Button variant="secondary" size="sm" onClick={() => { void copy(); }}>
                {copied ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
                {copied ? t("connectMatrix.copied") : t("connectMatrix.copyUrl")}
              </Button>
            </span>
            {copied && <span role="status" className="sr-only">{t("remoteAccess.linkCopied")}</span>}
          </li>
          <li>{t("remoteAccess.otherAppsStep3")}</li>
        </ol>
        <p className="remote-guide-example">{t("remoteAccess.otherAppsExample")}</p>
        <div className="pairing-approval-actions">
          <Button variant="primary" onClick={onClose}>{t("remoteAccess.done")}</Button>
        </div>
      </div>
    </div>
  );
}
