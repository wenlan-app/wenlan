// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { CodeBasisRef } from "../../../lib/tauri";
import { CodeBasisDialog } from "./CodeBasisDialog";
import { codeBasisLineText } from "./codeBasisShared";
import { OverflowMenu } from "./OverflowMenu";
import { codeBasisKeyOf, codeBasisNeedingAttention, type useCodeBasisActions } from "./useCodeBasisActions";

export interface NoteCodeBasisHandlers {
  actions: ReturnType<typeof useCodeBasisActions>;
  /** Open the editor so that the next save also confirms these lines. */
  onEditToConfirm: (row: CodeBasisRef) => void;
}

interface CodeBasisLinesProps extends NoteCodeBasisHandlers {
  pageId: string;
  codeBasis: readonly CodeBasisRef[];
  disabled?: boolean;
}

/** The code a note relies on that needs a look: one line each, with what can be done about it. */
export function CodeBasisLines({
  pageId,
  codeBasis,
  disabled = false,
  actions,
  onEditToConfirm,
}: CodeBasisLinesProps) {
  const { t } = useTranslation();
  const [viewing, setViewing] = useState<CodeBasisRef | null>(null);
  const lines = codeBasisNeedingAttention(codeBasis);
  if (lines.length === 0) return null;

  return <>
    <ul className="note-review-notice__basis-list" aria-label={t("codeBasis.groupLabel")}>
      {lines.map((row) => {
        const busy = actions.isBusy(pageId, row);
        const failure = actions.failureOf(pageId, row);
        const stop = () => { void actions.stop(pageId, row); };
        return <li key={codeBasisKeyOf(pageId, row)} className="note-review-notice__basis">
          <p className="note-review-notice__message note-review-notice__basis-text">{codeBasisLineText(t, row)}</p>
          <div className="note-review-notice__actions">
            {row.state === "pending" ? <>
              <button
                type="button"
                className="note-review-notice__button"
                disabled={disabled || busy}
                onClick={() => setViewing(row)}
              >{t("codeBasis.seeCode")}</button>
              <button
                type="button"
                className="note-review-notice__button"
                disabled={disabled || busy}
                onClick={() => { void actions.confirm(pageId, row); }}
              >{t("codeBasis.confirmNoImpact")}</button>
              <OverflowMenu
                label={t("codeBasis.moreActions")}
                disabled={disabled || busy}
                items={[
                  { key: "edit", label: t("codeBasis.editYourself"), onSelect: () => onEditToConfirm(row) },
                  { key: "stop", label: t("codeBasis.stopRelying"), onSelect: stop },
                ]}
              />
            </> : <button
              type="button"
              className="note-review-notice__button note-review-notice__button--quiet"
              disabled={disabled || busy}
              onClick={stop}
            >{t("codeBasis.stopRelying")}</button>}
          </div>
          {actions.hasChangedAgain(pageId, row) && <p className="note-review-notice__basis-note" role="status">
            {t("codeBasis.changedAgain")}
          </p>}
          {failure && <p className="note-review-notice__basis-note" role="alert">
            {t(failure === "confirm" ? "codeBasis.confirmFailed" : "codeBasis.stopFailed")}
          </p>}
        </li>;
      })}
    </ul>
    {viewing && <CodeBasisDialog
      pageId={pageId}
      row={viewing}
      confirming={actions.isBusy(pageId, viewing)}
      onClose={() => setViewing(null)}
      onConfirm={() => {
        const target = viewing;
        void actions.confirm(pageId, target).then(() => setViewing(null));
      }}
    />}
  </>;
}

export default CodeBasisLines;
