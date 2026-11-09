// SPDX-License-Identifier: AGPL-3.0-only
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Toggle } from "../settings/primitives";
import "./PageLockSwitch.css";

/** "Summary view": show only the short layer of a page that has two. It
 *  sits at the top of Info rather than in the page's menu. */
export function SummaryViewSwitch({ on, onChange }: {
  on: boolean;
  onChange: (on: boolean) => void;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  return <section className="page-lock page-summary-switch" aria-labelledby={titleId}>
    <div className="page-lock-row">
      <h3 id={titleId} className="note-info-section-heading page-lock-title">{t("pageLayers.summaryView")}</h3>
      <Toggle enabled={on} aria-label={t("pageLayers.summaryView")} onToggle={() => onChange(!on)} />
    </div>
  </section>;
}
