// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useRef, useState } from "react";
import { ChatsCircle, Eye, PencilSimpleLine } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { setExampleWikiActive } from "../../lib/exampleWiki/mode";
import { dragStripHeight } from "../../lib/windowChrome";
import { SourceTypeIcon } from "../memory/sources/SourceTypeIcon";
import { setStartIntent } from "./startIntent";
import "../memory/sources/source-access.css";
import "./notesWelcome.css";

/** The four ways in, as option rows: each row does its thing directly. */
const STARTS = [
  { id: "folder", icon: <SourceTypeIcon kind="folder" /> },
  { id: "chats", icon: <ChatsCircle aria-hidden="true" size={20} weight="regular" data-resource-icon="chats" /> },
  { id: "empty", icon: <PencilSimpleLine aria-hidden="true" size={20} weight="regular" data-resource-icon="empty" /> },
  { id: "example", icon: <Eye aria-hidden="true" size={20} weight="regular" data-resource-icon="example" /> },
] as const;

type Start = (typeof STARTS)[number]["id"];

interface NotesWelcomeProps {
  onComplete: () => void | Promise<void>;
}

export function NotesWelcome({ onComplete }: NotesWelcomeProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);

  async function start(way: Start) {
    // State updates may be batched, so guard before invoking the callback.
    if (inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setFailed(false);
    // The example opens as its own wiki; the other ways happen once the app
    // is open (the folder picker, past chats, a new page).
    if (way === "example") setExampleWikiActive(true);
    else setStartIntent(way);
    try {
      await onComplete();
    } catch {
      if (way === "example") setExampleWikiActive(false);
      setFailed(true);
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  }

  return (
    <div className="notes-welcome" data-testid="notes-welcome">
      <div
        data-tauri-drag-region
        style={{ height: dragStripHeight(), flexShrink: 0 }}
      />
      <main className="notes-welcome__main" aria-labelledby={titleId}>
        <div className="notes-welcome__content">
          <h1 id={titleId} className="notes-welcome__title">
            {t("setup.welcomeTitle")}
          </h1>
          <p className="notes-welcome__body">{t("setup.notesFirst.welcome.lead")}</p>
          {failed && (
            <p className="notes-welcome__error" role="alert">
              {t("setup.notesFirst.openFailed")}
            </p>
          )}
          <div className="notes-welcome__starts">
            {STARTS.map(({ id, icon }) => (
              <button
                key={id}
                type="button"
                className="source-add-option"
                data-testid={`notes-welcome-${id}`}
                disabled={pending}
                aria-busy={pending}
                onClick={() => void start(id)}
              >
                {icon}
                <span className="source-add-option-copy">
                  <strong>{t(`setup.notesFirst.welcome.${id}.title`)}</strong>
                  <span>{t(`setup.notesFirst.welcome.${id}.hint`)}</span>
                </span>
              </button>
            ))}
          </div>
          <p className="notes-welcome__foot">{t("setup.notesFirst.welcome.foot")}</p>
        </div>
      </main>
    </div>
  );
}
