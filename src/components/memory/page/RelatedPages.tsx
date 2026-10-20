// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import type { BasisRef, PageLinkOutbound } from "../../../lib/tauri";
import { PageIcon } from "../assets/PageIcon";
import { useTranslation } from "react-i18next";
import { OverflowMenu } from "./OverflowMenu";
import "./NoteInfo.css";

interface RelatedPagesProps {
  outbound: PageLinkOutbound[];
  onPageClick?: (pageId: string) => void;
  /** The note these links belong to; it can never be its own source. */
  currentPageId?: string;
  /** The notes the current note relies on. */
  basis?: readonly BasisRef[];
  onMarkBasis?: (pageId: string) => Promise<boolean>;
  onUnmarkBasis?: (pageId: string) => void;
}

const BADGE_TIP_SEEN_KEY = "wenlan.noteBasis.badgeTipSeen.v1";

function badgeTipSeen(): boolean {
  try {
    return window.localStorage.getItem(BADGE_TIP_SEEN_KEY) === "1";
  } catch {
    return false;
  }
}

function rememberBadgeTip() {
  try {
    window.localStorage.setItem(BADGE_TIP_SEEN_KEY, "1");
  } catch {
    // Storage can be blocked; the tip then simply shows again next time.
  }
}

export default function RelatedPages({
  outbound,
  onPageClick,
  currentPageId,
  basis,
  onMarkBasis,
  onUnmarkBasis,
}: RelatedPagesProps) {
  const { t } = useTranslation();
  // Which badge is showing the one-time tip, if any.
  const [tipFor, setTipFor] = useState<string | null>(null);
  const [markFailedFor, setMarkFailedFor] = useState<string | null>(null);
  // An empty "Related pages" header is noise, not information.
  if (outbound.length === 0) return null;

  const basisIds = new Set((basis ?? []).map((entry) => entry.page_id));
  const showTip = (pageId: string) => {
    if (badgeTipSeen()) return;
    rememberBadgeTip();
    setTipFor(pageId);
  };

  return (
    <section aria-label={t("knowledgeContext.linkedPages")} className="memory-detail-rail-section note-info-related-pages">
      <h3 className="note-info-section-heading">
        {t("knowledgeContext.linkedPages")}
      </h3>
      <div className="note-info-link-list">
        {outbound.map((link, idx) => {
          const key = `${link.label}-${link.target_page_id ?? idx}`;
          const inner = (
            <span className="note-info-link-copy">
              <PageIcon />
              <span className="note-info-link-label">{link.target_page_id ? link.target_title || link.label : link.label}</span>
            </span>
          );
          const targetPageId = link.target_page_id;
          if (!targetPageId || !onPageClick) {
            return (
              <div key={key} className="note-info-link-row is-unresolved" title={t("pageInfo.unresolvedLink")}>
                {inner}
                <span className="note-info-link-status">{t("pageInfo.unresolvedLink")}</span>
              </div>
            );
          }
          const isBasis = basisIds.has(targetPageId);
          const canMark = !isBasis && Boolean(onMarkBasis) && targetPageId !== currentPageId;
          const canUnmark = isBasis && Boolean(onUnmarkBasis);
          const row = (
            <button
              type="button"
              onClick={() => onPageClick(targetPageId)}
              className="note-info-link-row"
            >
              {inner}
            </button>
          );
          if (!isBasis && !canMark) return <div key={key} className="note-info-link-item">{row}</div>;
          return (
            <div key={key} className="note-info-link-item">
              {row}
              {isBasis && <span
                className="note-basis-badge"
                tabIndex={0}
                onMouseEnter={() => showTip(targetPageId)}
                onFocus={() => showTip(targetPageId)}
                onMouseLeave={() => setTipFor(null)}
                onBlur={() => setTipFor(null)}
              >
                {t("noteBasis.badge")}
                {tipFor === targetPageId && <span role="tooltip" className="note-basis-badge__tip">
                  {t("noteBasis.badgeTip")}
                </span>}
              </span>}
              {(canMark || canUnmark) && <OverflowMenu
                label={t("noteBasis.moreActions")}
                items={canUnmark
                  ? [{ key: "stop", label: t("noteBasis.stopUsing"), onSelect: () => onUnmarkBasis?.(targetPageId) }]
                  : [{
                    key: "use",
                    label: t("noteBasis.useAsSource"),
                    hint: t("noteBasis.useAsSourceHint"),
                    onSelect: () => {
                      setMarkFailedFor(null);
                      void onMarkBasis?.(targetPageId).then((ok) => {
                        if (!ok) setMarkFailedFor(targetPageId);
                      });
                    },
                  }]}
              />}
              {markFailedFor === targetPageId && <p role="alert" className="note-info-link-error">
                {t("noteBasis.markFailed")}
              </p>}
            </div>
          );
        })}
      </div>
    </section>
  );
}
