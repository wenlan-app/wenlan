// SPDX-License-Identifier: AGPL-3.0-only
import type { CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import { pageSupport, type PageTruth } from "../../lib/tauri";

interface PageTruthBadgesProps {
  readonly cutoverLive: boolean;
  readonly truth?: PageTruth | null;
  readonly wrapperClassName?: string;
  readonly wrapperStyle?: CSSProperties;
}

/**
 * Renders both M5 truth axes for one page: support (supported, unsupported,
 * or not checked yet)
 * and human review (reviewed/unreviewed). Renders nothing before the
 * daemon's cutover goes live, and nothing for a page with no `truth` — a
 * badge without a real axis value would be a guess, not a status, so this
 * component never invents one. `cutoverLive` comes from `useTruthStatus`.
 */
export function PageTruthBadges({ cutoverLive, truth, wrapperClassName, wrapperStyle }: PageTruthBadgesProps) {
  const { t } = useTranslation();
  if (!cutoverLive || !truth) {
    return null;
  }

  const support = pageSupport(truth);
  const supportLabel = t(`pages.overview.truth.${support}`);
  const reviewLabel = truth.human_reviewed
    ? t("pages.overview.truth.reviewed")
    : t("pages.overview.truth.unreviewed");

  const badges = (
    <>
      <span
        aria-label={`${t("pages.overview.truth.supportAxis")}: ${supportLabel}`}
        className={`wiki-page-state wiki-page-state--truth-${support}`}
        data-testid="page-truth-support"
      >
        {supportLabel}
      </span>
      <span
        aria-label={`${t("pages.overview.truth.humanReviewAxis")}: ${reviewLabel}`}
        className={`wiki-page-state wiki-page-state--truth-${truth.human_reviewed ? "reviewed" : "unreviewed"}`}
        data-testid="page-truth-review"
      >
        {reviewLabel}
      </span>
    </>
  );

  if (wrapperClassName || wrapperStyle) {
    return (
      <span className={wrapperClassName} style={wrapperStyle}>
        {badges}
      </span>
    );
  }
  return badges;
}
