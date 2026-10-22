// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import {
  ArrowsClockwise,
  Check,
  Code,
  Copy,
  MagnifyingGlass,
  Play,
  PushPin,
  X,
} from "@phosphor-icons/react";
import { useLiveAgents } from "../../connect/connectionState";
import { writeClipboard } from "../page/codeBasisShared";
import {
  ASK_AI_TOOLS,
  askAiCommand,
  askAiTasksFor,
  defaultAskAiTool,
  type AskAiTaskId,
  type AskAiTool,
} from "./askAiCommands";
import "./askAiDialog.css";

const COPIED_MS = 2_000;

const ICONS: Record<AskAiTaskId, React.ReactNode> = {
  capture: <PushPin weight="regular" />,
  recall: <MagnifyingGlass weight="regular" />,
  brief: <Play weight="regular" />,
  steep: <ArrowsClockwise weight="regular" />,
  wiki: <Code weight="regular" />,
};

interface AskAiDialogProps {
  readonly onClose: () => void;
  /** Opens the connect flow. Offered only while no AI tool is connected. */
  readonly onConnectTool?: () => void;
}

/** "What your AI can do": the tasks a user can hand their AI tool, written
 *  the way that tool takes them, each one copyable. */
export function AskAiDialog({ onClose, onConnectTool }: AskAiDialogProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const agents = useLiveAgents();
  const fallback = defaultAskAiTool(agents.data ?? []);
  // The tab follows the roster until the user picks one.
  const [picked, setPicked] = useState<AskAiTool | null>(null);
  const tool = picked ?? fallback.tool;
  const [copiedId, setCopiedId] = useState<AskAiTaskId | null>(null);
  const [failedId, setFailedId] = useState<AskAiTaskId | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    panelRef.current?.focus();
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const tasks = askAiTasksFor(tool);
  const groups = (["daily", "sometimes"] as const).map((group) => ({
    group,
    tasks: tasks.filter((task) => task.group === group),
  }));

  const copy = async (id: AskAiTaskId, text: string) => {
    if (await writeClipboard(text)) {
      setFailedId(null);
      setCopiedId(id);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopiedId(null), COPIED_MS);
    } else {
      setFailedId(id);
    }
  };

  return createPortal(
    <div
      className="ask-ai-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="ask-ai-panel"
        data-testid="ask-ai-dialog"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onClose();
            return;
          }
          if (event.key !== "Tab") return;
          const controls = Array.from(panelRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? []);
          const first = controls[0];
          const last = controls[controls.length - 1];
          if (event.shiftKey && (document.activeElement === first || document.activeElement === panelRef.current)) {
            event.preventDefault();
            last?.focus();
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault();
            first?.focus();
          }
        }}
      >
        <header className="ask-ai-header">
          <h2 id={titleId}>{t("askAi.title")}</h2>
          <button type="button" className="ask-ai-close" aria-label={t("askAi.close")} title={t("askAi.close")} onClick={onClose}>
            <X aria-hidden="true" weight="regular" />
          </button>
        </header>
        {!fallback.connected && onConnectTool ? (
          <p className="ask-ai-connect" data-testid="ask-ai-connect">
            {t("askAi.connectFirst")}{" "}
            <button type="button" className="ask-ai-link" onClick={onConnectTool}>
              {t("askAi.connect")}
            </button>
          </p>
        ) : null}
        <div className="ask-ai-tabs" role="tablist" aria-label={t("askAi.toolsLabel")}>
          {ASK_AI_TOOLS.map((id) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={id === tool}
              className="ask-ai-tab"
              data-testid={`ask-ai-tab-${id}`}
              onClick={() => setPicked(id)}
            >
              {t(`askAi.tools.${id}`)}
            </button>
          ))}
        </div>
        <div role="tabpanel">
          <p className="ask-ai-lead">{t(`askAi.lead.${tool}`)}</p>
          {groups.map(({ group, tasks: groupTasks }) => (
            <section key={group} className="ask-ai-group" aria-label={t(`askAi.groups.${group}`)}>
              <h3>{t(`askAi.groups.${group}`)}</h3>
              <ul>
                {groupTasks.map((task) => {
                  const text = askAiCommand(task, tool, {
                    example: t(`askAi.tasks.${task.id}.example`),
                    sentence: t(`askAi.tasks.${task.id}.sentence`),
                  });
                  const title = t(`askAi.tasks.${task.id}.title`);
                  const copied = copiedId === task.id;
                  return (
                    <li key={task.id} data-testid={`ask-ai-task-${task.id}`}>
                      <span className="ask-ai-icon" aria-hidden="true">{ICONS[task.id]}</span>
                      <div className="ask-ai-copy">
                        <p className="ask-ai-task-title">{title}</p>
                        <code className={tool === "other" ? "ask-ai-command ask-ai-command--plain" : "ask-ai-command"}>{text}</code>
                        {failedId === task.id ? (
                          <p role="alert" className="ask-ai-copy-error">{t("askAi.copyFailed")}</p>
                        ) : null}
                      </div>
                      <button
                        type="button"
                        className="ask-ai-copy-button"
                        aria-label={`${copied ? t("askAi.copied") : t("askAi.copy")}: ${title}`}
                        title={copied ? t("askAi.copied") : t("askAi.copy")}
                        onClick={() => void copy(task.id, text)}
                      >
                        {copied ? <Check aria-hidden="true" weight="bold" /> : <Copy aria-hidden="true" weight="regular" />}
                      </button>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
          {tool !== "other" ? <p className="ask-ai-tip">{t(`askAi.tip.${tool}`)}</p> : null}
        </div>
      </div>
    </div>,
    document.body,
  );
}
