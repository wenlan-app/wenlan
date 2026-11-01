// SPDX-License-Identifier: AGPL-3.0-only
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Toggle } from "../settings/primitives";
import "./PageLockSwitch.css";

/**
 * The page's one Lock switch (docs/ux/spec.md, UX-WHO-2), off by default.
 * Off, the AI keeps its own untouched sentences up to date and proposes
 * changes to the person's. On, every AI change waits for the person. There is
 * no "may edit directly" mode. The switch is the page's review lock.
 */
export function PageLockSwitch({ locked, pending, disabled, onChange }: {
  locked: boolean;
  pending: boolean;
  disabled: boolean;
  onChange: (locked: boolean) => void;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const hintId = useId();
  return <section className="page-lock" aria-labelledby={titleId}>
    <div className="page-lock-row">
      <h3 id={titleId} className="note-info-section-heading page-lock-title">{t("pageLock.title")}</h3>
      <Toggle
        enabled={locked}
        valueUnknown={pending}
        unknownState="saving"
        aria-label={t("pageLock.title")}
        aria-describedby={hintId}
        onToggle={() => { if (!disabled) onChange(!locked); }}
      />
    </div>
    <p id={hintId} className="page-lock-hint">{t(locked ? "pageLock.onHint" : "pageLock.offHint")}</p>
  </section>;
}
