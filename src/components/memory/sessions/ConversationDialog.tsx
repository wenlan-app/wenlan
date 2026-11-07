// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { agentDisplayName, getConversationLines, type ConversationLine } from "../../../lib/tauri";
import { readable } from "../SearchResults";
import { useSessionDetail } from "./SessionCard";
import { SessionResumeAction } from "./SessionResumeAction";
import "./sessionSources.css";

interface ConversationDialogProps {
  readonly sessionId: string;
  /** The line to open at and mark, as `/api/sessions/{id}/lines` counts. */
  readonly focusIndex: number | null;
  readonly onClose: () => void;
}

/**
 * One whole conversation, read like the AI tool it came from, open at the
 * line a sentence came from (UX-WHO-6, UX-FIND-2).
 */
export function ConversationDialog({ sessionId, focusIndex, onClose }: ConversationDialogProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const linesRef = useRef<HTMLDivElement>(null);
  const focusRef = useRef<HTMLParagraphElement>(null);
  const lines = useQuery({
    queryKey: ["conversationLines", sessionId],
    queryFn: () => getConversationLines(sessionId),
    retry: false,
  });
  const detail = useSessionDetail(sessionId);
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    panel.current?.focus();
    return () => { if (previous?.isConnected) previous.focus(); };
  }, []);
  useEffect(() => {
    const box = linesRef.current;
    const line = focusRef.current;
    if (lines.data && box && line) box.scrollTop = line.offsetTop - (box.clientHeight - line.offsetHeight) / 2;
  }, [lines.data]);
  const tool = lines.data?.tool ?? detail.data?.session.tool ?? "";
  const speaker = (line: ConversationLine) =>
    line.speaker === "you" ? t("main.search.you") : agentDisplayName(tool) ?? tool;
  const title = lines.data?.title ?? detail.data?.session.title ?? lines.data?.thing_title ?? agentDisplayName(tool) ?? "";

  return (
    <div className="session-card-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div
        ref={panel}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="session-card-panel conversation-dialog"
        data-testid="conversation-dialog"
        onKeyDown={(event) => {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(); }
        }}
      >
        <div className="session-card-body">
          <h2 id={titleId} className="session-card-title">{title}</h2>
          {lines.isPending && <p role="status" className="session-card-muted">{t("thisSentence.said.loading")}</p>}
          {lines.isError && <p role="alert" className="session-card-muted">{t("main.search.conversationGone")}</p>}
          {lines.data && (
            <div className="search-quote-lines search-quote-lines--chat conversation-dialog-lines" ref={linesRef}>
              {lines.data.lines.map((line, i) => {
                const at = i + (lines.data.first_index ?? 0);
                const focus = at === focusIndex;
                return (
                  <p
                    className={focus ? "search-quote-line search-quote-line--hit" : "search-quote-line"}
                    data-speaker={line.speaker}
                    key={at}
                    ref={focus ? focusRef : undefined}
                  >
                    <span className="search-quote-speaker">{speaker(line)}</span>
                    <span className="search-quote-text">{readable(line.text)}</span>
                  </p>
                );
              })}
            </div>
          )}
          <div className="conversation-dialog-actions">
            {detail.data && (
              <SessionResumeAction
                session={detail.data.session}
                transcriptAvailable={detail.data.transcript_available === true}
              />
            )}
            <button type="button" className="search-quote-link" onClick={onClose}>{t("thisSentence.said.close")}</button>
          </div>
        </div>
      </div>
    </div>
  );
}
