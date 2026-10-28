// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useLayoutEffect, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { X } from "@phosphor-icons/react";
import "./tabIntro.css";

/** The tabs whose name alone does not say what is in them. */
export type IntroTab = "memories" | "sources" | "topics" | "graph" | "spaces" | "activity";

const SEEN_KEY = "onboarding:tabIntros";

function readSeen(): Set<string> {
  try {
    const stored = JSON.parse(localStorage.getItem(SEEN_KEY) ?? "[]");
    return new Set(Array.isArray(stored) ? stored.filter((tab) => typeof tab === "string") : []);
  } catch {
    return new Set();
  }
}

function markSeen(tab: IntroTab) {
  try {
    const seen = readSeen();
    seen.add(tab);
    localStorage.setItem(SEEN_KEY, JSON.stringify([...seen]));
  } catch {
    // Storage refused: the line shows again next time, which is harmless.
  }
}

/** Settings' "Show again" brings the explanations back with the page tips. */
export function resetTabIntros() {
  try {
    localStorage.removeItem(SEEN_KEY);
  } catch {
    // Nothing stored to clear.
  }
}

/**
 * One quiet line, the first time a new user opens a tab, saying what the
 * tab's name means ("Topics: people, places and things your pages mention").
 * It floats at the foot of the content area, centred on it, so no view moves,
 * and it does not come back once the user has left the tab or closed it.
 */
export function TabIntro({ tab, newUser, areaRef }: {
  readonly tab: IntroTab | null;
  readonly newUser: boolean;
  /** The content area the line is centred under. */
  readonly areaRef: RefObject<HTMLElement | null>;
}) {
  const { t } = useTranslation();
  const [closed, setClosed] = useState<IntroTab | null>(null);
  // Decided when the tab opens, so marking it seen does not hide it at once.
  const [shown, setShown] = useState<IntroTab | null>(null);
  useEffect(() => {
    if (!tab || !newUser || readSeen().has(tab)) {
      setShown(null);
      return;
    }
    setShown(tab);
    // Leaving the tab counts as having read it.
    return () => markSeen(tab);
  }, [tab, newUser]);

  const visible = shown !== null && shown === tab && closed !== shown;
  const [centre, setCentre] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (!visible) return;
    const place = () => {
      const rect = areaRef.current?.getBoundingClientRect();
      setCentre(rect ? rect.left + rect.width / 2 : null);
    };
    place();
    window.addEventListener("resize", place);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    if (areaRef.current) observer?.observe(areaRef.current);
    return () => {
      window.removeEventListener("resize", place);
      observer?.disconnect();
    };
  }, [visible, areaRef]);

  if (!visible) return null;
  return createPortal(
    <div className="tab-intro-anchor" style={centre === null ? undefined : { left: centre }}>
      <p className="tab-intro" role="note" data-testid="tab-intro" data-tab={shown}>
        <span>{t(`tabIntro.${shown}`)}</span>
        <button
          type="button"
          className="tab-intro-close"
          aria-label={t("tabIntro.close")}
          onClick={() => {
            markSeen(shown);
            setClosed(shown);
          }}
        >
          <X size={12} weight="bold" aria-hidden="true" />
        </button>
      </p>
    </div>,
    document.body,
  );
}
