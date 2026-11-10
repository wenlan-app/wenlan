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
  const found = results.flatMap((r, i) => (r.data ? [{ ...r.data, sourceId: ids[i] }] : []));
  if (found.length === 0) return null;
  // Kept lines are only the copy the AI tool sent; there is no conversation
  // behind them to open.
  const opens = (said: (typeof found)[number]) => !said.kept && !!said.session_id;
  const keyOf = (said: (typeof found)[number], index: number) =>
    opens(said) ? `${said.session_id}:${index}` : `${said.sourceId}:kept:${index}`;
  const seen = new Set<string>();
  const lines = found
    .flatMap((said) => said.lines.map((line) => ({ said, line })))
    .filter(({ said, line }) => {
      const key = keyOf(said, line.index);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .slice(0, SHOWN);
  const openable = found.find(opens);
  const firstOpenable = lines.find(({ said }) => opens(said));
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
          {lines.map(({ said, line }) => {
            const body = (
              <>
                <span className="said-line-meta">
                  <span>{speaker(line.line, said.tool)}</span>
                  {line.line.at ? <span>{relativeTime(line.line.at, t, i18n.language)}</span> : null}
                </span>
                <span className="said-line-text">{readable(line.line.text)}</span>
              </>
            );
            const sessionId = said.session_id;
            return (
              <li key={keyOf(said, line.index)}>
                {opens(said) && sessionId ? (
                  <button
                    type="button"
                    className="said-line"
                    data-speaker={line.line.speaker}
                    onClick={() => setOpen({ sessionId, index: line.index })}
                  >
                    {body}
                  </button>
                ) : (
                  <div className="said-line" data-speaker={line.line.speaker} data-kept="true">
                    {body}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
        </>
      )}
      {openable?.session_id ? (
        <button
          type="button"
          className="said-lines-all"
          onClick={() =>
            setOpen({
              sessionId: (firstOpenable?.said ?? openable).session_id ?? openable.session_id!,
              index: firstOpenable?.line.index ?? null,
            })
          }
        >
          {t("thisSentence.said.showAll")}
        </button>
      ) : null}
      {open && <ConversationDialog sessionId={open.sessionId} focusIndex={open.index} onClose={() => setOpen(null)} />}
    </div>
  );
}
