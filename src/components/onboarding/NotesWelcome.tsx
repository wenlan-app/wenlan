// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { ChatsCircle, Eye, PencilSimpleLine } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { dragStripHeight } from "../../lib/windowChrome";
import { SourceTypeIcon } from "../memory/sources/SourceTypeIcon";
import { FirstUseSample } from "./FirstUseSample";
import { setStartIntent, type StartIntent } from "./startIntent";
import "../memory/sources/source-access.css";
import "./notesWelcome.css";

/** The four ways in, as option rows: each row does its thing directly. */
const STARTS = [
  { id: "folder", icon: <SourceTypeIcon kind="folder" /> },
  { id: "chats", icon: <ChatsCircle aria-hidden="true" size={20} weight="regular" data-resource-icon="chats" /> },
  { id: "empty", icon: <PencilSimpleLine aria-hidden="true" size={20} weight="regular" data-resource-icon="empty" /> },
  { id: "example", icon: <Eye aria-hidden="true" size={20} weight="regular" data-resource-icon="example" /> },
] as const;

interface NotesWelcomeProps {
  onComplete: () => void | Promise<void>;
}

export function NotesWelcome({ onComplete }: NotesWelcomeProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const [sampleOpen, setSampleOpen] = useState(false);
  const sampleTrigger = useRef<HTMLButtonElement | null>(null);
  const sampleHost = useRef<HTMLDivElement | null>(null);
  const restoreSampleFocus = useRef(false);

  useEffect(() => {
    if (sampleOpen) {
      sampleHost.current?.focus();
    } else if (restoreSampleFocus.current) {
      restoreSampleFocus.current = false;
      sampleTrigger.current?.focus();
    }
  }, [sampleOpen]);

  async function openNotes(intent?: StartIntent) {
    // State updates may be batched, so guard before invoking the callback.
    if (inFlight.current) return;
    if (intent) setStartIntent(intent);
    inFlight.current = true;
    setPending(true);
    setFailed(false);
    try {
      await onComplete();
    } catch {
      setFailed(true);
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  }

  function backToWelcome() {
    if (inFlight.current) return;
    restoreSampleFocus.current = true;
    setFailed(false);
    setSampleOpen(false);
  }

  return (
    <div className="notes-welcome" data-testid="notes-welcome">
      <div
        data-tauri-drag-region
        style={{ height: dragStripHeight(), flexShrink: 0 }}
      />
      <main
        className={sampleOpen
          ? "notes-welcome__main notes-welcome__main--sample"
          : "notes-welcome__main"}
        aria-labelledby={sampleOpen ? undefined : titleId}
        aria-label={sampleOpen ? t("firstUse.sample.animationLabel") : undefined}
      >
        {sampleOpen ? (
          <div className="notes-welcome__sample">
            {failed && (
              <p className="notes-welcome__error" role="alert">
                {t("setup.notesFirst.openFailed")}
              </p>
            )}
            <div
              ref={sampleHost}
              tabIndex={-1}
              inert={pending ? true : undefined}
              aria-busy={pending}
              onClickCapture={(event) => {
                if (inFlight.current) {
                  event.preventDefault();
                  event.stopPropagation();
                }
              }}
            >
              <FirstUseSample
                backLabel={t("setup.back")}
                onBack={backToWelcome}
                finishLabel={t("setup.notesFirst.start")}
                onFinish={() => void openNotes()}
              />
            </div>
          </div>
        ) : (
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
                  ref={id === "example" ? sampleTrigger : undefined}
                  type="button"
                  className="source-add-option"
                  data-testid={`notes-welcome-${id}`}
                  disabled={pending}
                  aria-busy={pending}
                  onClick={() => {
                    if (inFlight.current) return;
                    setFailed(false);
                    if (id === "example") setSampleOpen(true);
                    else void openNotes(id);
                  }}
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
        )}
      </main>
    </div>
  );
}
