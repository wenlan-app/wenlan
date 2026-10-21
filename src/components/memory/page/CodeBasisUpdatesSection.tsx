// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { CodeBasisPendingEntry, CodeBasisRef } from "../../../lib/tauri";
import { CodeBasisDialog } from "./CodeBasisDialog";
import { codeBasisLineText } from "./codeBasisShared";
import { OverflowMenu } from "./OverflowMenu";
import { codeBasisKeyOf, codeBasisNeedingAttention, useCodeBasisActions, useEverydayModelAvailable } from "./useCodeBasisActions";
import "./BasisUpdates.css";

/** The review destination for "notes whose code changed". */
export const CODE_BASIS_UPDATES_SECTION_ID = "review-code-basis-updates";

interface CodeBasisUpdatesSectionProps {
  entries: readonly CodeBasisPendingEntry[];
  /** Scroll this group into view when the review was opened for it. */
  focusOnMount?: boolean;
  onPageClick?: (pageId: string) => void;
  headingStyle?: React.CSSProperties;
  countStyle?: React.CSSProperties;
}

/**
 * Review group for notes whose code changed: one row per note, opening to the
 * lines it relies on that need a look, each with the same actions as the
 * notice on the note itself (editing the note is done from the note).
 */
export function CodeBasisUpdatesSection({
  entries,
  focusOnMount = false,
  onPageClick,
  headingStyle,
  countStyle,
}: CodeBasisUpdatesSectionProps) {
  const { t } = useTranslation();
  const actions = useCodeBasisActions();
  const modelAvailable = useEverydayModelAvailable();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [viewing, setViewing] = useState<{ pageId: string; row: CodeBasisRef } | null>(null);
  const sectionRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (focusOnMount && entries.length > 0) sectionRef.current?.scrollIntoView?.({ block: "start" });
  }, [focusOnMount, entries.length]);

  if (entries.length === 0) return null;

  const toggle = (pageId: string) => setExpanded((current) => {
    const next = new Set(current);
    if (next.has(pageId)) next.delete(pageId);
    else next.add(pageId);
    return next;
  });

  return <section
    ref={sectionRef}
    id={CODE_BASIS_UPDATES_SECTION_ID}
    tabIndex={-1}
    aria-label={t("codeBasis.review.group")}
    className="basis-updates"
  >
    <h2 style={headingStyle}>
      {t("codeBasis.review.group")}
      <span aria-hidden="true" style={countStyle}>{entries.length}</span>
    </h2>
    <ul className="basis-updates__list">
      {entries.map((entry) => {
        const open = expanded.has(entry.page_id);
        const rows = codeBasisNeedingAttention(entry.pending, modelAvailable);
        return <li key={entry.page_id} className="basis-updates__group">
          <div className="basis-updates__summary">
            <button
              type="button"
              className="basis-updates__link"
              aria-expanded={open}
              onClick={() => toggle(entry.page_id)}
            >{entry.title}</button>
            <span className="basis-updates__actions">
              <span className="basis-updates__minor">{t("codeBasis.codeCount", { count: rows.length })}</span>
              {onPageClick && <button
                type="button"
                className="basis-updates__button"
                onClick={() => onPageClick(entry.page_id)}
              >{t("codeBasis.review.openNote")}</button>}
            </span>
          </div>
          {open && <ul className="basis-updates__citers">
            {rows.map((row) => {
              const busy = actions.isBusy(entry.page_id, row);
              const failure = actions.failureOf(entry.page_id, row);
              const stop = () => { void actions.stop(entry.page_id, row); };
              return <li key={codeBasisKeyOf(entry.page_id, row)} className="basis-updates__citer">
                <span className="basis-updates__citer-title">{codeBasisLineText(t, row)}</span>
                <span className="basis-updates__actions">
                  {row.state === "pending" ? <>
                    <button
                      type="button"
                      className="basis-updates__button"
                      disabled={busy}
                      onClick={() => setViewing({ pageId: entry.page_id, row })}
                    >{t("codeBasis.seeCode")}</button>
                    <button
                      type="button"
                      className="basis-updates__button"
                      disabled={busy}
                      onClick={() => { void actions.confirm(entry.page_id, row); }}
                    >{t("codeBasis.confirmNoImpact")}</button>
                    <OverflowMenu
                      label={t("codeBasis.moreActions")}
                      disabled={busy}
                      items={[{ key: "stop", label: t("codeBasis.stopRelying"), onSelect: stop }]}
                    />
                  </> : <button
                    type="button"
                    className="basis-updates__button"
                    disabled={busy}
                    onClick={stop}
                  >{t("codeBasis.stopRelying")}</button>}
                </span>
                {actions.hasChangedAgain(entry.page_id, row) && <p className="basis-updates__note" role="status">
                  {t("codeBasis.changedAgain")}
                </p>}
                {failure && <p className="basis-updates__note" role="alert">
                  {t(failure === "confirm" ? "codeBasis.confirmFailed" : "codeBasis.stopFailed")}
                </p>}
              </li>;
            })}
          </ul>}
        </li>;
      })}
    </ul>
    {viewing && <CodeBasisDialog
      pageId={viewing.pageId}
      row={viewing.row}
      confirming={actions.isBusy(viewing.pageId, viewing.row)}
      onClose={() => setViewing(null)}
      onConfirm={() => {
        const target = viewing;
        void actions.confirm(target.pageId, target.row).then(() => setViewing(null));
      }}
    />}
  </section>;
}

export default CodeBasisUpdatesSection;
