// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { X } from "@phosphor-icons/react";
import "./tabIntro.css";

/** Where the content area's middle is, so a line floating at its foot sits
 *  centred on it rather than on the window. Shared with the example wiki's line. */
export function useAreaCentre(areaRef: RefObject<HTMLElement | null>, active: boolean): number | null {
  const [centre, setCentre] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (!active) return;
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
  }, [active, areaRef]);
  return centre;
}

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

// Tabs showing their own empty state right now. An empty tab already says
// what it is and what to do there, so the line waits until it has content
// rather than putting a second explanation under the first.
const emptyTabs = new Set<IntroTab>();
const emptyListeners = new Set<() => void>();
function subscribeEmpty(listener: () => void) {
  emptyListeners.add(listener);
  return () => {
    emptyListeners.delete(listener);
  };
}

/** Rendered inside a tab's empty state, so that state is the one explanation. */
export function TabEmptyMark({ tab }: { readonly tab: IntroTab }) {
  useEffect(() => {
    emptyTabs.add(tab);
    emptyListeners.forEach((listener) => listener());
    return () => {
      emptyTabs.delete(tab);
      emptyListeners.forEach((listener) => listener());
    };
  }, [tab]);
  return null;
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
 * One quiet line, the first time a new user opens a tab with something in
 * it, saying what the tab's name means ("Topics: people, places and things
 * your pages mention").
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
  const empty = useSyncExternalStore(subscribeEmpty, () => tab !== null && emptyTabs.has(tab), () => false);
  // Only a line the user actually saw counts as read.
  const displayed = useRef(false);
  useEffect(() => {
    displayed.current = false;
    if (!tab || !newUser || readSeen().has(tab)) {
      setShown(null);
      return;
    }
    setShown(tab);
    // Leaving the tab counts as having read it.
    return () => {
      if (displayed.current) markSeen(tab);
    };
  }, [tab, newUser]);

  const visible = shown !== null && shown === tab && closed !== shown && !empty;
  useEffect(() => {
    if (visible) displayed.current = true;
  }, [visible]);
  const centre = useAreaCentre(areaRef, visible);

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
