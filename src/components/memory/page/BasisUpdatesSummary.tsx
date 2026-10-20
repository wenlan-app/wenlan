// SPDX-License-Identifier: AGPL-3.0-only
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { useBasisPending } from "./useBasisActions";
import "./NoteReviewNotice.css";
import "./BasisUpdates.css";

interface BasisUpdatesSummaryProps {
  /** Open the review group these lines point at. */
  onOpen: () => void;
}

/**
 * Home: a compact, neutral notice (the same family as the note review notice)
 * with one line per source note that changed and a visible "Review" action
 * that opens the review group. Nothing at all when no note is waiting. It asks
 * for its own data so that the home shell stays free of this read.
 */
export function BasisUpdatesSummary({ onOpen }: BasisUpdatesSummaryProps) {
  const { t } = useTranslation();
  const { entries } = useBasisPending();
  const idPrefix = useId();
  if (entries.length === 0) return null;

  return <div className="note-review-notice basis-updates-home">
    <ul className="note-review-notice__basis-list" aria-label={t("noteBasis.review.group")}>
      {entries.map((entry) => {
        const textId = `${idPrefix}-${entry.basis_page_id}`;
        return <li key={entry.basis_page_id} className="note-review-notice__basis">
          <span id={textId} className="note-review-notice__basis-text">
            {t(entry.citers.length === 1 ? "noteBasis.review.summary_one" : "noteBasis.review.summary_other", {
              title: entry.title || t("noteBasis.untitled"),
              count: entry.citers.length,
            })}
          </span>
          <button
            type="button"
            className="note-review-notice__button"
            aria-describedby={textId}
            onClick={onOpen}
          >
            {t("noteBasis.review.action")}
          </button>
        </li>;
      })}
    </ul>
  </div>;
}

export default BasisUpdatesSummary;
