// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import type { CodeBasisRef } from "../../../lib/tauri";
import { CodeBasisDialog } from "./CodeBasisDialog";
import { codeBasisAiLineText } from "./codeBasisShared";
import { codeBasisKeyOf, type useCodeBasisActions } from "./useCodeBasisActions";

export interface NoteCodeBasisHandlers {
  actions: ReturnType<typeof useCodeBasisActions>;
  /** Open the editor so that the next save also confirms these lines. */
  onEditToConfirm: (row: CodeBasisRef) => void;
}

interface CodeBasisLinesProps extends NoteCodeBasisHandlers {
  pageId: string;
  /** The rows that need a person, as `codeBasisNeedingAttention` picks them. */
  codeBasis: readonly CodeBasisRef[];
  disabled?: boolean;
}

/** The notice's one line: the file when there is one row, a count when there are more. */
function summaryText(t: TFunction, rows: readonly CodeBasisRef[]): string {
  if (rows.length > 1) return t("codeBasis.notice.many", { count: rows.length });
  const [row] = rows;
  return t(row.state === "pending" ? "codeBasis.notice.changedOne" : "codeBasis.notice.missingOne", { file: row.path });
}

/**
 * The code a note relies on that needs a look: one line and one button. The
 * decisions are made in the dialog, one row after another, where the code can
 * be seen; once every row is settled the line goes away.
 */
export function CodeBasisLines({
  pageId,
  codeBasis,
  disabled = false,
  actions,
  onEditToConfirm,
}: CodeBasisLinesProps) {
  const { t } = useTranslation();
  // The rows as they stood when the dialog opened: settling one refetches the
  // page and drops it from `codeBasis`, and the step must not jump.
  const [queue, setQueue] = useState<readonly CodeBasisRef[] | null>(null);
  const [index, setIndex] = useState(0);
  // Bumped when the code changed again, so the dialog reads the lines afresh.
  const [attempt, setAttempt] = useState(0);
  // Which button the person pressed, so only that one spins.
  const [doing, setDoing] = useState<"confirm" | "stop" | null>(null);
  const current = queue?.[index] ?? null;
  if (codeBasis.length === 0 && !current) return null;

  const close = () => { setQueue(null); setIndex(0); };
  const next = () => {
    if (!queue || index + 1 >= queue.length) close();
    else setIndex(index + 1);
  };

  let message: { text: string; alert: boolean } | null = null;
  if (current) {
    const failure = actions.failureOf(pageId, current);
    if (failure) message = { text: t(failure === "confirm" ? "codeBasis.confirmFailed" : "codeBasis.stopFailed"), alert: true };
    else if (actions.hasChangedAgain(pageId, current)) message = { text: t("codeBasis.changedAgain"), alert: false };
  }

  return <>
    {codeBasis.length > 0 && <div className="note-review-notice__basis note-review-notice__code-summary">
      <p className="note-review-notice__message note-review-notice__basis-text">{summaryText(t, codeBasis)}</p>
      <div className="note-review-notice__actions">
        <button
          type="button"
          className="note-review-notice__button"
          disabled={disabled}
          onClick={() => { setQueue([...codeBasis]); setIndex(0); }}
        >{t("codeBasis.notice.review")}</button>
      </div>
    </div>}
    {current && queue && <CodeBasisDialog
      key={`${codeBasisKeyOf(pageId, current)}\u0000${attempt}`}
      pageId={pageId}
      row={current}
      confirming={actions.isBusy(pageId, current) && doing === "confirm"}
      onClose={close}
      onConfirm={() => {
        const target = current;
        setDoing("confirm");
        void actions.confirm(pageId, target).then((answer) => {
          if (answer === "confirmed") next();
          else if (answer === "conflict") setAttempt((value) => value + 1);
        });
      }}
      decide={{
        step: { index, total: queue.length },
        note: codeBasisAiLineText(t, current),
        message,
        stopping: actions.isBusy(pageId, current) && doing === "stop",
        onStop: () => {
          const target = current;
          setDoing("stop");
          void actions.stop(pageId, target).then((done) => { if (done) next(); });
        },
        onEdit: () => {
          const target = current;
          close();
          onEditToConfirm(target);
        },
        onLater: next,
      }}
    />}
  </>;
}

export default CodeBasisLines;
