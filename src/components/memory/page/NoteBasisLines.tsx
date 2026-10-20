// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { BasisRef } from "../../../lib/tauri";
import { BasisDiffDialog } from "./BasisDiffDialog";
import { OverflowMenu } from "./OverflowMenu";
import type { useBasisActions } from "./useBasisActions";

export interface NoteBasisHandlers {
  actions: ReturnType<typeof useBasisActions>;
  /** Open the editor so that the next save also confirms this basis. */
  onEditToConfirm: (basis: BasisRef) => void;
  onUpdateFromSources: () => void;
  /** "Update from sources" has nothing to work from. */
  updateFromSourcesDisabled?: boolean;
}

interface NoteBasisLinesProps extends NoteBasisHandlers {
  pageId: string;
  basis: readonly BasisRef[];
  disabled?: boolean;
}

/** The bases that need a look: one line each, with what can be done about it. */
export function basisNeedingAttention(basis: readonly BasisRef[] | undefined): BasisRef[] {
  return (basis ?? []).filter((entry) => entry.state !== "ok");
}

export function NoteBasisLines({
  pageId,
  basis,
  disabled = false,
  actions,
  onEditToConfirm,
  onUpdateFromSources,
  updateFromSourcesDisabled = false,
}: NoteBasisLinesProps) {
  const { t } = useTranslation();
  const [diffFor, setDiffFor] = useState<BasisRef | null>(null);
  const lines = basisNeedingAttention(basis);
  if (lines.length === 0) return null;

  return <>
    <ul className="note-review-notice__basis-list">
      {lines.map((entry) => {
        const title = entry.title || t("noteBasis.untitled");
        const busy = actions.isBusy(pageId, entry.page_id);
        const stop = () => { void actions.stop(pageId, entry.page_id); };
        const text = entry.state === "pending"
          ? `${t("noteBasis.pendingLine", { title })}${entry.minor_only ? ` ${t("noteBasis.pendingMinor")}` : ""}`
          : entry.state === "archived"
            ? t("noteBasis.staleArchived", { title })
            : entry.state === "deleted"
              ? t("noteBasis.staleDeleted", { title })
              : t("noteBasis.staleUnavailable", { title });
        return <li key={entry.page_id} className="note-review-notice__basis">
          <p className="note-review-notice__message note-review-notice__basis-text">{text}</p>
          <div className="note-review-notice__actions">
            {entry.state === "pending" ? <>
              <button
                type="button"
                className="note-review-notice__button"
                disabled={disabled || busy}
                onClick={() => setDiffFor(entry)}
              >{t("noteBasis.seeChanges")}</button>
              <button
                type="button"
                className="note-review-notice__button"
                disabled={disabled || busy}
                onClick={() => {
                  if (entry.to_version == null) {
                    setDiffFor(entry);
                    return;
                  }
                  void actions.confirm(pageId, entry.page_id, entry.to_version);
                }}
              >{t("noteBasis.confirmNoImpact")}</button>
              <OverflowMenu
                label={t("noteBasis.moreActions")}
                disabled={disabled || busy}
                items={[
                  { key: "edit", label: t("noteBasis.editYourself"), onSelect: () => onEditToConfirm(entry) },
                  {
                    key: "update",
                    label: t("noteBasis.updateFromSources"),
                    disabled: updateFromSourcesDisabled,
                    onSelect: onUpdateFromSources,
                  },
                  { key: "stop", label: t("noteBasis.stopUsing"), onSelect: stop },
                ]}
              />
            </> : <button
              type="button"
              className="note-review-notice__button note-review-notice__button--quiet"
              disabled={disabled || busy}
              onClick={stop}
            >{t("noteBasis.stopUsing")}</button>}
          </div>
          {actions.hasChangedAgain(pageId, entry.page_id) && <p className="note-review-notice__basis-note" role="status">
            {t("noteBasis.changedAgain", { title })}
          </p>}
          {actions.hasFailed(pageId, entry.page_id) && <p className="note-review-notice__basis-note" role="alert">
            {t(entry.state === "pending" ? "noteBasis.confirmFailed" : "noteBasis.stopFailed")}
          </p>}
        </li>;
      })}
    </ul>
    {diffFor && <BasisDiffDialog
      pageId={pageId}
      basisPageId={diffFor.page_id}
      basisTitle={diffFor.title}
      confirming={actions.isBusy(pageId, diffFor.page_id)}
      onClose={() => setDiffFor(null)}
      onConfirm={(version) => {
        const target = diffFor;
        void actions.confirm(pageId, target.page_id, version).then(() => setDiffFor(null));
      }}
    />}
  </>;
}

export default NoteBasisLines;
