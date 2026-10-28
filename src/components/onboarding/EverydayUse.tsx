// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { X } from "@phosphor-icons/react";
import { AskInToolButton, frontDoorFor } from "../memory/askAi/askInTool";
import { ASK_AI_ICONS, AskAiDialog } from "../memory/askAi/AskAiDialog";
import { useHoldOnboardingStage } from "./onboardingStage";

import "./everydayUse.css";

const CLOSED_KEY = "onboarding:everydayUse";
const listeners = new Set<() => void>();

function readClosed(): boolean {
  try {
    return localStorage.getItem(CLOSED_KEY) === "closed";
  } catch {
    return false;
  }
}

function close() {
  try {
    localStorage.setItem(CLOSED_KEY, "closed");
  } catch {
    // Storage refused: the card closes for this session only.
  }
  closedThisSession = true;
  listeners.forEach((listener) => listener());
}

let closedThisSession = false;

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The day's rhythm, in the order it happens: start, while working, wrap up,
 *  each with the plugin skill that does the same thing. */
const MOMENTS = [
  { id: "start", skill: "brief" },
  { id: "remember", skill: "capture" },
  { id: "wrapUp", skill: "handoff" },
] as const;

/**
 * The moment the checklist's three steps are done, while the library is still
 * new: one overlay, centred over the app, with the three things a person says
 * to their AI tool on an ordinary day (where did I leave off, remember this,
 * wrap up), each one click away from that tool. It is the one turning point
 * worth interrupting for, so it shows once: ×, Esc or a click outside closes
 * it for good.
 */
export function EverydayUse({ tool }: { readonly tool: { readonly family: string; readonly name: string } }) {
  const { t } = useTranslation();
  const titleId = useId();
  const panel = useRef<HTMLElement | null>(null);
  const closed = useSyncExternalStore(subscribe, () => closedThisSession || readClosed(), () => false);
  // "See all commands" swaps this for the full list; closing that closes both.
  const [more, setMore] = useState(false);
  useHoldOnboardingStage(!closed);
  useEffect(() => {
    if (!closed && !more) panel.current?.focus();
  }, [closed, more]);
  if (closed) return null;
  if (more) return <AskAiDialog onClose={close} />;
  const frontDoor = frontDoorFor(tool.family);
  return createPortal(
    <div
      className="everyday-use-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    >
    <section
      ref={panel}
      tabIndex={-1}
      role="dialog"
      aria-modal="true"
      className="getting-started everyday-use"
      data-testid="everyday-use"
      aria-labelledby={titleId}
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        close();
      }}
    >
      <header className="getting-started-header">
        <h2 id={titleId}>{t("everydayUse.title")}</h2>
        <button
          type="button"
          className="getting-started-hide"
          aria-label={t("everydayUse.close")}
          title={t("everydayUse.close")}
          onClick={close}
        >
          <X aria-hidden="true" weight="regular" />
        </button>
      </header>
      <p className="getting-started-lead">
        {t("everydayUse.lead", { tool: tool.name })}{" "}
        <button type="button" className="getting-started-link" data-testid="everyday-use-more" onClick={() => setMore(true)}>
          {t("everydayUse.more")}
        </button>
      </p>
      <ul className="everyday-use-rows">
        {MOMENTS.map(({ id: moment, skill }) => {
          // A tool with the plugin takes the front door and a few words, and
          // names the skill that does the same; any other tool is asked in a
          // plain sentence.
          const text = frontDoor
            ? `${frontDoor} ${t(`everydayUse.moments.${moment}.words`)}`
            : t(`everydayUse.moments.${moment}.sentence`);
          // Codex calls plugin skills with `$`, Claude Code with `/`.
          const skillText = frontDoor ? `${frontDoor.charAt(0)}${skill}` : null;
          return (
            <li key={moment} data-testid={`everyday-use-${moment}`}>
              <span className="ask-ai-icon" aria-hidden="true">{ASK_AI_ICONS[skill]}</span>
              <div className="everyday-use-copy">
                {/* When, then what it keeps: "remember" keeps one thing, "wrap up"
                    the whole chat, so the two read as different. */}
                <p>
                  <strong>{t(`everydayUse.moments.${moment}.when`)}</strong>{" "}
                  {t(`everydayUse.moments.${moment}.does`)}
                </p>
                <code className={frontDoor ? undefined : "everyday-use-plain"}>{text}</code>
                {skillText ? (
                  <span className="everyday-use-skill">
                    {t("everydayUse.orSkill")} <code>{skillText}</code>
                  </span>
                ) : null}
              </div>
              <AskInToolButton
                family={tool.family}
                tool={tool.name}
                text={text}
                label={t("askAi.tryIn", { tool: tool.name })}
                className="ask-in-tool everyday-use-action"
                testId={`everyday-use-try-${moment}`}
              />
            </li>
          );
        })}
      </ul>
    </section>
    </div>,
    document.body,
  );
}
