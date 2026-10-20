// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { BasisPendingCiter, BasisPendingEntry } from "../../../lib/tauri";
import { BasisDiffDialog } from "./BasisDiffDialog";
import { OverflowMenu } from "./OverflowMenu";
import { useBasisActions } from "./useBasisActions";
import "./BasisUpdates.css";

interface BasisUpdatesSectionProps {
  entries: readonly BasisPendingEntry[];
  /** Scroll this group into view when the review was opened for it. */
  focusOnMount?: boolean;
  onPageClick?: (pageId: string) => void;
  headingStyle?: React.CSSProperties;
  countStyle?: React.CSSProperties;
}

/**
 * Review group for notes whose source note changed: one row per source note,
 * opening to the notes that rely on it, each with the same actions as the
 * notice on the note itself.
 */
export function BasisUpdatesSection({
  entries,
  focusOnMount = false,
  onPageClick,
  headingStyle,
  countStyle,
}: BasisUpdatesSectionProps) {
  const { t } = useTranslation();
  const actions = useBasisActions();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [stopFailed, setStopFailed] = useState<ReadonlySet<string>>(new Set());
  const [diffFor, setDiffFor] = useState<{ pageId: string; entry: BasisPendingEntry } | null>(null);
  const sectionRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (focusOnMount && entries.length > 0) sectionRef.current?.scrollIntoView?.({ block: "start" });
  }, [focusOnMount, entries.length]);

  if (entries.length === 0) return null;

  const toggle = (basisPageId: string) => setExpanded((current) => {
    const next = new Set(current);
    if (next.has(basisPageId)) next.delete(basisPageId);
    else next.add(basisPageId);
    return next;
  });

  const confirm = (entry: BasisPendingEntry, citer: BasisPendingCiter) => {
    if (entry.changed_version == null) {
      setDiffFor({ pageId: citer.page_id, entry });
      return;
    }
    void actions.confirm(citer.page_id, entry.basis_page_id, entry.changed_version);
  };

  return <section
    ref={sectionRef}
    id="review-basis-updates"
    tabIndex={-1}
    aria-label={t("noteBasis.review.group")}
    className="basis-updates"
  >
    <h2 style={headingStyle}>
      {t("noteBasis.review.group")}
      <span aria-hidden="true" style={countStyle}>{entries.length}</span>
    </h2>
    <ul className="basis-updates__list">
      {entries.map((entry) => {
        const open = expanded.has(entry.basis_page_id);
        const title = entry.title || t("noteBasis.untitled");
        return <li key={entry.basis_page_id} className="basis-updates__group">
          <div className="basis-updates__summary">
            <p className="basis-updates__text">
              {t(entry.citers.length === 1 ? "noteBasis.review.summary_one" : "noteBasis.review.summary_other", {
                title,
                count: entry.citers.length,
              })}
            </p>
            <button
              type="button"
              className="basis-updates__button"
              aria-expanded={open}
              onClick={() => toggle(entry.basis_page_id)}
            >{t(open ? "noteBasis.review.collapse" : "noteBasis.review.expand")}</button>
          </div>
          {open && <ul className="basis-updates__citers">
            {entry.citers.map((citer) => {
              const busy = actions.isBusy(citer.page_id, entry.basis_page_id);
              return <li key={citer.page_id} className="basis-updates__citer">
                <span className="basis-updates__citer-title">
                  {onPageClick
                    ? <button type="button" className="basis-updates__link" onClick={() => onPageClick(citer.page_id)}>
                      {citer.title}
                    </button>
                    : citer.title}
                  {citer.minor_only && <span className="basis-updates__minor">{t("noteBasis.pendingMinor")}</span>}
                </span>
                <span className="basis-updates__actions">
                  <button
                    type="button"
                    className="basis-updates__button"
                    disabled={busy}
                    onClick={() => setDiffFor({ pageId: citer.page_id, entry })}
                  >{t("noteBasis.seeChanges")}</button>
                  <button
                    type="button"
                    className="basis-updates__button"
                    disabled={busy}
                    onClick={() => confirm(entry, citer)}
                  >{t("noteBasis.confirmNoImpact")}</button>
                  {onPageClick && <button
                    type="button"
                    className="basis-updates__button"
                    onClick={() => onPageClick(citer.page_id)}
                  >{t("noteBasis.openNote")}</button>}
                  <OverflowMenu
                    label={t("noteBasis.moreActions")}
                    disabled={busy}
                    items={[{
                      key: "stop",
                      label: t("noteBasis.stopUsing"),
                      onSelect: () => {
                        const key = `${citer.page_id}\u0000${entry.basis_page_id}`;
                        void actions.stop(citer.page_id, entry.basis_page_id).then((done) => setStopFailed((current) => {
                          const next = new Set(current);
                          if (done) next.delete(key);
                          else next.add(key);
                          return next;
                        }));
                      },
                    }]}
                  />
                </span>
                {actions.hasChangedAgain(citer.page_id, entry.basis_page_id) && <p className="basis-updates__note" role="status">
                  {t("noteBasis.changedAgain", { title })}
                </p>}
                {actions.hasFailed(citer.page_id, entry.basis_page_id) && <p className="basis-updates__note" role="alert">
                  {t(stopFailed.has(`${citer.page_id}\u0000${entry.basis_page_id}`) ? "noteBasis.stopFailed" : "noteBasis.confirmFailed")}
                </p>}
              </li>;
            })}
          </ul>}
        </li>;
      })}
    </ul>
    {diffFor && <BasisDiffDialog
      pageId={diffFor.pageId}
      basisPageId={diffFor.entry.basis_page_id}
      basisTitle={diffFor.entry.title}
      confirming={actions.isBusy(diffFor.pageId, diffFor.entry.basis_page_id)}
      onClose={() => setDiffFor(null)}
      onConfirm={(version) => {
        const target = diffFor;
        void actions.confirm(target.pageId, target.entry.basis_page_id, version).then(() => setDiffFor(null));
      }}
    />}
  </section>;
}

export default BasisUpdatesSection;
