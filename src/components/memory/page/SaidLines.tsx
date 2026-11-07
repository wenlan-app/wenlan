// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useQueries } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { agentDisplayName, getMemorySaid, type ConversationLine } from "../../../lib/tauri";
import { relativeTime } from "../../../lib/relativeTime";
import { readable } from "../SearchResults";
import { ConversationDialog } from "../sessions/ConversationDialog";
import "./saidLines.css";

/** Lines shown at most, whatever number of memories a sentence cites. */
const SHOWN = 3;
/** Memories looked up at most for one sentence. */
const LOOKED_UP = 3;

export const memorySaidKey = (sourceId: string) => ["memory-said", sourceId] as const;

interface SaidLinesProps {
  /** The memories this sentence or point came from (their `source_id`). */
  readonly sourceIds: readonly string[];
}

/**
 * The lines of the conversation a sentence came from: who said each and
 * when, each opening the whole conversation there (UX-WHO-6). The daemon
 * picks the lines; nothing shows when no memory was saved in a conversation
 * Wenlan can read. Finding a line is not checking it, so nothing here looks
 * verified (UX-WHO-5).
 */
export function SaidLines({ sourceIds }: SaidLinesProps) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState<{ sessionId: string; index: number | null } | null>(null);
  const ids = [...new Set(sourceIds)].slice(0, LOOKED_UP);
  const results = useQueries({
    queries: ids.map((id) => ({
      queryKey: memorySaidKey(id),
      queryFn: () => getMemorySaid(id),
      retry: false,
      staleTime: 60_000,
    })),
  });
  if (results.some((r) => r.isPending)) return null;
  const found = results.flatMap((r) => (r.data ? [r.data] : []));
  if (found.length === 0) return null;
  const seen = new Set<string>();
  const lines = found
    .flatMap((said) => said.lines.map((line) => ({ said, line })))
    .filter(({ said, line }) => {
      const key = `${said.session_id}:${line.index}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .slice(0, SHOWN);
  const speaker = (line: ConversationLine, tool: string) =>
    line.speaker === "you" ? t("main.search.you") : agentDisplayName(tool) ?? tool;

  return (
    <div className="said-lines" data-testid="said-lines">
      {/* "Not found in the conversation" already says where it looked. */}
      {lines.length === 0 ? (
        <p className="said-lines-none">{t("thisSentence.said.notFound")}</p>
      ) : (
        <>
        <p className="said-lines-title">{t("thisSentence.said.title")}</p>
        <ul className="said-lines-list">
          {lines.map(({ said, line }) => (
            <li key={`${said.session_id}:${line.index}`}>
              <button
                type="button"
                className="said-line"
                data-speaker={line.line.speaker}
                onClick={() => setOpen({ sessionId: said.session_id, index: line.index })}
              >
                <span className="said-line-meta">
                  <span>{speaker(line.line, said.tool)}</span>
                  {line.line.at ? <span>{relativeTime(line.line.at, t, i18n.language)}</span> : null}
                </span>
                <span className="said-line-text">{readable(line.line.text)}</span>
              </button>
            </li>
          ))}
        </ul>
        </>
      )}
      <button
        type="button"
        className="said-lines-all"
        onClick={() => setOpen({ sessionId: (lines[0]?.said ?? found[0]).session_id, index: lines[0]?.line.index ?? null })}
      >
        {t("thisSentence.said.showAll")}
      </button>
      {open && <ConversationDialog sessionId={open.sessionId} focusIndex={open.index} onClose={() => setOpen(null)} />}
    </div>
  );
}
