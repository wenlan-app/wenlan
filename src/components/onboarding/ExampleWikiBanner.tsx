// SPDX-License-Identifier: AGPL-3.0-only
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { setExampleWikiActive, useExampleWikiActive } from "../../lib/exampleWiki/mode";
import { useAreaCentre } from "./TabIntro";
import "../memory/askAi/turnIntoPages.css";
import "./exampleWikiBanner.css";

/**
 * Opens and closes the example wiki. Every cached read is reset on the way in
 * and out, so the example's pages and the user's own never show together.
 */
export function useExampleWikiSwitch() {
  const queryClient = useQueryClient();
  const switchTo = useCallback((on: boolean) => {
    setExampleWikiActive(on);
    void queryClient.resetQueries();
  }, [queryClient]);
  return {
    open: useCallback(() => switchTo(true), [switchTo]),
    close: useCallback(() => switchTo(false), [switchTo]),
  };
}

/**
 * One line at the foot of the content area while the example wiki is open,
 * in the same floating pill as the first-visit tab lines (which move up above
 * it): what it is, a way back, and the one main action, starting your own.
 */
export function ExampleWikiBanner({ onLeave, areaRef }: {
  /** Leaves the example. The workspace owns the switch: it saves both editor
   *  groups first, and `startOwn` opens "Add your material" on the way back. */
  readonly onLeave: (startOwn: boolean) => void;
  readonly areaRef: RefObject<HTMLElement | null>;
}) {
  const { t } = useTranslation();
  const active = useExampleWikiActive();
  const centre = useAreaCentre(areaRef, active);
  if (!active) return null;
  return createPortal(
    <div className="example-wiki-anchor" style={centre === null ? undefined : { left: centre }}>
    <div className="example-wiki-banner" role="region" aria-label={t("exampleWiki.label")} data-testid="example-wiki-banner">
      <p>
        <strong>{t("exampleWiki.title")}</strong>
        <span>{t("exampleWiki.line")}</span>
      </p>
      <div className="example-wiki-banner-actions">
        <button type="button" className="getting-started-link" data-testid="example-wiki-back" onClick={() => onLeave(false)}>
          {t("exampleWiki.back")}
        </button>
        <button type="button" className="turn-pages-primary" data-testid="example-wiki-start" onClick={() => onLeave(true)}>
          {t("exampleWiki.startOwn")}
        </button>
      </div>
    </div>
    </div>,
    document.body,
  );
}
