// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, Sparkle, X } from "@phosphor-icons/react";
import { useLiveAgents } from "../../connect/connectionState";
import { writeClipboard } from "../page/codeBasisShared";
import { ASK_AI_TASKS, askAiCommand, defaultAskAiTool, toolOfFamily, type AskAiTaskId } from "./askAiCommands";
import "./askAiHint.css";

const COPIED_MS = 2_000;

/** The text to type for one task in the user's own tool, or null when that
 *  tool cannot do it (a skill-only task in a tool without the plugin), or
 *  while the connected tools are still being read. */
export function useAskAiCommand(taskId: AskAiTaskId, family?: string | null): string | null {
  const { t } = useTranslation();
  const agents = useLiveAgents();
  // Until the roster is read the tool is unknown; naming Claude Code's command
  // first and swapping it a moment later would be worse than waiting.
  if (!agents.isSuccess) return null;
  // A tool the user picked words the command its own way.
  const tool = family ? toolOfFamily(family) : defaultAskAiTool(agents.data).tool;
  const task = ASK_AI_TASKS.find((candidate) => candidate.id === taskId);
  if (!task || (tool === "other" && task.skillOnly)) return null;
  return askAiCommand(task, tool, {
    example: t(`askAi.tasks.${taskId}.example`),
    sentence: t(`askAi.tasks.${taskId}.sentence`),
  });
}

/** A copy button for one command; shows "Copied" for two seconds. */
export function AskAiCopyButton({ text, label }: { readonly text: string; readonly label?: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  const word = copied ? t("askAi.copied") : t("askAi.copy");
  return (
    <button
      type="button"
      className="ask-ai-hint-copy"
      aria-label={label ? `${word}: ${label}` : word}
      title={word}
      onClick={() => {
        void writeClipboard(text).then((ok) => {
          if (!ok) return;
          setCopied(true);
          if (timer.current) clearTimeout(timer.current);
          timer.current = setTimeout(() => setCopied(false), COPIED_MS);
        });
      }}
    >
      {copied ? <Check aria-hidden="true" weight="bold" /> : <Copy aria-hidden="true" weight="regular" />}
    </button>
  );
}

interface AskAiHintProps {
  readonly task: AskAiTaskId;
  /** What just happened and what typing the command does, ending where the
   *  command follows. */
  readonly lead: string;
  readonly onDismiss?: () => void;
  /** Opens the full "What your AI can do" list. */
  readonly onSeeAll?: () => void;
  readonly testId?: string;
}

/** One quiet line at the moment a command is useful: what to type, and a copy
 *  button. No banner, no toast; it sits in the page it belongs to. */
export function AskAiHint({ task, lead, onDismiss, onSeeAll, testId }: AskAiHintProps) {
  const { t } = useTranslation();
  const command = useAskAiCommand(task);
  if (command === null) return null;
  return (
    <div className="ask-ai-hint" data-testid={testId} role="note">
      <Sparkle aria-hidden="true" weight="regular" className="ask-ai-hint-icon" />
      <p className="ask-ai-hint-text">
        {lead} <code className="ask-ai-hint-command">{command}</code>
      </p>
      <AskAiCopyButton text={command} />
      {onSeeAll ? (
        <button type="button" className="ask-ai-hint-more" onClick={onSeeAll}>{t("askAi.hints.seeAll")}</button>
      ) : null}
      {onDismiss ? (
        <button type="button" className="ask-ai-hint-dismiss" aria-label={t("askAi.hide")} title={t("askAi.hide")} onClick={onDismiss}>
          <X aria-hidden="true" weight="regular" />
        </button>
      ) : null}
    </div>
  );
}

const DISMISSED_PREFIX = "askAi:hintDismissed:";

/** Per-viewer memory of a hint the user closed. Storage may be unavailable;
 *  then the hint stays closed for this session only. */
export function useHintDismissed(key: string): [boolean, () => void] {
  const storageKey = `${DISMISSED_PREFIX}${key}`;
  const read = () => {
    try {
      return localStorage.getItem(storageKey) === "1";
    } catch {
      return false;
    }
  };
  const [dismissed, setDismissed] = useState(read);
  useEffect(() => setDismissed(read()), [storageKey]); // eslint-disable-line react-hooks/exhaustive-deps
  return [dismissed, () => {
    try {
      localStorage.setItem(storageKey, "1");
    } catch {
      // Closed for this session only.
    }
    setDismissed(true);
  }];
}
