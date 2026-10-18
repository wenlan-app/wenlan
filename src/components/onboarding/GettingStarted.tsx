// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  CheckCircle,
  FolderSimplePlus,
  NotePencil,
  PlugsConnected,
  Sparkle,
  X,
} from "@phosphor-icons/react";
import { clipboardWrite, getBackgroundAiEnabled, openToolWithPrompt, PROMPT_LINK_FAMILIES } from "../../lib/tauri";
import { resolveAgentDisplayName } from "../../lib/agents";
import {
  connectedToolFamilies,
  GETTING_STARTED_ORDER,
  publishedPages,
  useGettingStartedState,
  type GettingStartedStepId,
} from "./useGettingStartedState";
import "./gettingStarted.css";

const COPIED_MS = 2_000;

async function copyText(text: string): Promise<boolean> {
  try {
    await clipboardWrite(text);
    return true;
  } catch {
    // Outside Tauri (browser preview) the plugin is missing; try the web API.
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export interface GettingStartedProps {
  readonly onWriteNote: () => void;
  readonly onConnectTool: () => void;
  readonly onAddSources: () => void;
  readonly onSeeExample: () => void;
  /** Opens the setting that turns background steeping on. Without it the
   *  optional steeping line is not offered. */
  readonly onTurnOnSteeping?: () => void;
}

export function GettingStarted({
  onWriteNote,
  onConnectTool,
  onAddSources,
  onSeeExample,
  onTurnOnSteeping,
}: GettingStartedProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  // "opened": the tool has the question typed in. "copied": it could not be
  // opened, so the question is on the clipboard to paste there instead.
  const [handedOver, setHandedOver] = useState<"opened" | "copied" | null>(null);
  const { visible, progress, doneCount, dismissed, setDismissed, pages, agents } = useGettingStartedState();
  // Same key as the Intelligence settings, so turning it on there hides the line here.
  const backgroundAi = useQuery({
    queryKey: ["backgroundAiEnabled"],
    queryFn: getBackgroundAiEnabled,
    enabled: visible && onTurnOnSteeping != null,
  });

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copied]);

  if (!visible || !progress) return null;

  const stepsById: Record<GettingStartedStepId, { icon: React.ReactNode; onAction: () => void }> = {
    note: { icon: <NotePencil weight="regular" />, onAction: onWriteNote },
    tool: { icon: <PlugsConnected weight="regular" />, onAction: onConnectTool },
    sources: { icon: <FolderSimplePlus weight="regular" />, onAction: onAddSources },
    recall: { icon: <Sparkle weight="regular" />, onAction: onSeeExample },
  };
  const order = GETTING_STARTED_ORDER;

  if (dismissed) {
    return (
      <button
        type="button"
        className="getting-started-reopen"
        onClick={() => setDismissed(false)}
      >
        {t("firstUse.checklist.reopen")}
      </button>
    );
  }

  // The moment the whole thing is for happens outside Wenlan, in the AI tool.
  // Once a note is saved and a tool is connected, open that tool with the
  // question about the note they just wrote already typed in (a tool that
  // cannot be opened that way gets it copied). The step then checks itself
  // when the tool reads the note.
  const published = publishedPages(pages);
  const families = connectedToolFamilies(agents);
  const promptFamily = families.find((family) => PROMPT_LINK_FAMILIES.includes(family)) ?? families[0];
  const opensTool = promptFamily != null && PROMPT_LINK_FAMILIES.includes(promptFamily);
  const promptTool = progress.note && progress.tool && !progress.recall && promptFamily != null
    ? resolveAgentDisplayName(promptFamily, agents)
    : null;
  const promptText = promptTool
    ? t("firstUse.checklist.steps.recall.prompt", { title: published[0].title })
    : null;
  const onlyDrafts = !progress.note && pages.length > 0;

  return (
    <section
      className="getting-started"
      data-testid="getting-started"
      aria-labelledby="getting-started-title"
    >
      <header className="getting-started-header">
        <h2 id="getting-started-title">{t("firstUse.checklist.title")}</h2>
        <button
          type="button"
          className="getting-started-hide"
          aria-label={t("firstUse.checklist.hide")}
          title={t("firstUse.checklist.hide")}
          onClick={() => setDismissed(true)}
        >
          <X aria-hidden="true" weight="regular" />
        </button>
      </header>
      <p className="getting-started-progress">
        {t("firstUse.checklist.progress", { done: doneCount, total: order.length })}
        {/* Once the ask row offers its own button, the sample moves up here. */}
        {progress.recall || promptText ? (
          <>
            {" · "}
            <button type="button" className="getting-started-link" onClick={onSeeExample}>
              {t("firstUse.checklist.steps.recall.action")}
            </button>
          </>
        ) : null}
      </p>
      <ol className="getting-started-steps">
        {order.map((id) => {
          const step = stepsById[id];
          const done = progress[id];
          // Once there is a question to ask, the recall row reads like the
          // others: the question as its one line, and one button that opens the
          // tool with it typed in (or copies it where that is not possible).
          const asking = !done && id === "recall" && promptTool != null && promptText != null;
          const body = id === "note" && onlyDrafts
            ? t("firstUse.checklist.steps.note.draftBody")
            : asking && handedOver === "opened"
              ? t("firstUse.checklist.steps.recall.waiting", { tool: promptTool })
              : asking && opensTool && handedOver !== "copied"
                // The button already names the tool, so the line is just the question.
                ? t("firstUse.checklist.steps.recall.question", { title: published[0].title })
                : asking
                  ? t("firstUse.checklist.steps.recall.askLine", { tool: promptTool, title: published[0].title })
              : t(`firstUse.checklist.steps.${id}.body`);
          return (
            <li
              key={id}
              data-testid={`getting-started-${id}`}
              data-done={done ? "true" : "false"}
            >
              <span className="getting-started-icon" aria-hidden="true">
                {done ? <CheckCircle weight="fill" /> : step.icon}
              </span>
              <div className="getting-started-copy">
                <p className="getting-started-step-title">
                  {t(`firstUse.checklist.steps.${id}.title`)}
                  {done ? (
                    <span className="sr-only">{` ${t("firstUse.checklist.done")}`}</span>
                  ) : null}
                </p>
                {done ? null : <p className="getting-started-step-body">{body}</p>}
              </div>
              {done ? null : asking ? (
                <button
                  type="button"
                  className="getting-started-action"
                  aria-live="polite"
                  onClick={() => {
                    void copyText(promptText).then(async (ok) => {
                      if (opensTool && promptFamily != null) {
                        try {
                          await openToolWithPrompt(promptFamily, promptText);
                          setHandedOver("opened");
                          return;
                        } catch {
                          // Not installed here; the copied question still works.
                        }
                      }
                      if (ok) {
                        setHandedOver("copied");
                        setCopied(true);
                      }
                    });
                  }}
                >
                  {copied
                    ? t("firstUse.checklist.steps.recall.copied")
                    : opensTool && handedOver !== "copied"
                      ? t("firstUse.checklist.steps.recall.askIn", { tool: promptTool })
                      : t("firstUse.checklist.steps.recall.copy")}
                </button>
              ) : (
                <button
                  type="button"
                  className="getting-started-action"
                  onClick={step.onAction}
                >
                  {t(`firstUse.checklist.steps.${id}.action`)}
                </button>
              )}
            </li>
          );
        })}
      </ol>
      {/* Optional, so it sits under the steps instead of being one of them.
          Shown only when the setting is known to be off. */}
      {onTurnOnSteeping && backgroundAi.data === false ? (
        <p className="getting-started-extra" data-testid="getting-started-steep">
          {t("firstUse.checklist.steep.line")}{" "}
          <button type="button" className="getting-started-link" onClick={onTurnOnSteeping}>
            {t("firstUse.checklist.steep.action")}
          </button>
        </p>
      ) : null}
    </section>
  );
}
