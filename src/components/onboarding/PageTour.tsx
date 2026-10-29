// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import "./pageTour.css";

/** "seen" once the tips were closed; "replay" when Settings asked for them
 *  again; absent before the first time. */
const TOUR_KEY = "onboarding:pageTour";
type TourState = "seen" | "replay" | null;

const listeners = new Set<() => void>();
/** Set only when storage rejected the last write, so it still holds this session. */
let inMemory: TourState | undefined;

function readTour(): TourState {
  if (inMemory !== undefined) return inMemory;
  try {
    const stored = localStorage.getItem(TOUR_KEY);
    return stored === "seen" || stored === "replay" ? stored : null;
  } catch {
    return null;
  }
}

function writeTour(value: TourState) {
  try {
    if (value) localStorage.setItem(TOUR_KEY, value);
    else localStorage.removeItem(TOUR_KEY);
    inMemory = undefined;
  } catch {
    inMemory = value;
  }
  listeners.forEach((listener) => listener());
}

function subscribeTour(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Settings' "Show again": the tips come back on the next page with sources. */
export function replayPageTour() {
  writeTour("replay");
}

export function usePageTourState(): TourState {
  return useSyncExternalStore(subscribeTour, readTour, () => null);
}

type TipId = "sources" | "unverified" | "edit";

/** Each tip points at the first match of its selectors, read view or editor,
 *  tried in order. A tip with nothing to point at is left out. */
const TIPS: ReadonlyArray<{ id: TipId; selectors: readonly string[] }> = [
  {
    id: "sources",
    selectors: [
      '.citation-chip:not([data-status="unverified"])',
      ".cm-citation-link:not(.reference-link--unverified)",
      ".citation-chip",
      ".cm-citation-link",
    ],
  },
  {
    id: "unverified",
    selectors: ['.citation-chip[data-status="unverified"]', ".cm-citation-link.reference-link--unverified"],
  },
  {
    id: "edit",
    selectors: [
      ".cm-content .cm-line:has(.cm-citation-link)",
      '[data-testid="page-document-reading"] p',
      ".cm-content",
      '[data-testid="page-document-reading"]',
    ],
  },
];

function findAnchor(root: HTMLElement, selectors: readonly string[]): HTMLElement | null {
  for (const selector of selectors) {
    try {
      const found = root.querySelector<HTMLElement>(selector);
      if (found) return found;
    } catch {
      // An engine without :has() skips to the next selector.
    }
  }
  return null;
}

/** How long to wait for the page and its sources to show before giving up. */
const LOOK_FOR_MS = 8_000;
const LOOK_EVERY_MS = 250;
const GAP = 10;
const EDGE = 16;
const WIDTH = 300;

interface Place {
  readonly top: number;
  readonly left: number;
  readonly ring: { top: number; left: number; width: number; height: number };
}

function placeNear(anchor: HTMLElement, card: HTMLElement | null): Place {
  const rect = anchor.getBoundingClientRect();
  const height = card?.offsetHeight ?? 120;
  const width = Math.min(WIDTH, window.innerWidth - EDGE * 2);
  const below = rect.bottom + GAP;
  const top = below + height <= window.innerHeight - EDGE || rect.top - GAP - height < EDGE
    ? below
    : rect.top - GAP - height;
  const left = Math.max(EDGE, Math.min(rect.left - 12, window.innerWidth - width - EDGE));
  return {
    top,
    left,
    ring: { top: rect.top - 3, left: rect.left - 3, width: rect.width + 6, height: rect.height + 6 },
  };
}

interface PageTourProps {
  /** The page's own element; tips only point inside it. */
  readonly rootRef: RefObject<HTMLElement | null>;
  /** A page Wenlan wrote with sources, open and ready to read or edit. */
  readonly eligible: boolean;
  /** Someone still getting started, who has not seen the tips yet. */
  readonly newUser: boolean;
  /** Changes when another page opens, so the search starts again. */
  readonly pageKey: string;
}

/** Up to three tips, one at a time, on the first page Wenlan writes: where a
 *  sentence's source is, what an unchecked one looks like, and that the page
 *  is the user's to change. Never covers the page or takes focus from it. */
export function PageTour({ rootRef, eligible, newUser, pageKey }: PageTourProps) {
  const { t } = useTranslation();
  const state = usePageTourState();
  const wanted = eligible && (state === "replay" || (state === null && newUser));
  const [tips, setTips] = useState<TipId[] | null>(null);
  const [index, setIndex] = useState(0);
  const [place, setPlace] = useState<Place | null>(null);
  const cardRef = useRef<HTMLDivElement | null>(null);
  const nextRef = useRef<HTMLButtonElement | null>(null);
  const movedByTour = useRef(false);

  useEffect(() => {
    setTips(null);
    setIndex(0);
    setPlace(null);
    movedByTour.current = false;
  }, [pageKey]);

  // Wait for the sources to show: they are what the tips are about.
  useEffect(() => {
    if (!wanted || tips) return;
    const started = Date.now();
    const look = () => {
      const root = rootRef.current;
      if (!root) return false;
      if (!findAnchor(root, TIPS[0]!.selectors)) return false;
      setTips(TIPS.filter((tip) => findAnchor(root, tip.selectors)).map((tip) => tip.id));
      return true;
    };
    if (look()) return;
    const timer = window.setInterval(() => {
      if (look() || Date.now() - started > LOOK_FOR_MS) window.clearInterval(timer);
    }, LOOK_EVERY_MS);
    return () => window.clearInterval(timer);
  }, [wanted, tips, rootRef, pageKey]);

  const current = wanted && tips ? tips[index] : undefined;
  const selectors = TIPS.find((tip) => tip.id === current)?.selectors;

  const reposition = useCallback(() => {
    const root = rootRef.current;
    const anchor = root && selectors ? findAnchor(root, selectors) : null;
    setPlace(anchor ? placeNear(anchor, cardRef.current) : null);
  }, [rootRef, selectors]);

  useLayoutEffect(() => {
    if (!selectors) return;
    const root = rootRef.current;
    const anchor = root ? findAnchor(root, selectors) : null;
    const rect = anchor?.getBoundingClientRect();
    if (anchor && rect && (rect.top < EDGE || rect.bottom > window.innerHeight - 160)) {
      anchor.scrollIntoView?.({ block: "center" });
    }
    reposition();
  }, [selectors, reposition]);

  useEffect(() => {
    if (!selectors) return;
    // The editor redraws its lines, so the anchor is looked up each time.
    const frame = { id: 0 };
    const schedule = () => {
      cancelAnimationFrame(frame.id);
      frame.id = requestAnimationFrame(reposition);
    };
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    const settle = window.setInterval(schedule, 500);
    return () => {
      cancelAnimationFrame(frame.id);
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      window.clearInterval(settle);
    };
  }, [selectors, reposition]);

  const finish = useCallback(() => writeTour("seen"), []);

  useEffect(() => {
    if (!current) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      finish();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [current, finish]);

  // Focus follows the tips only once the user is using them.
  useEffect(() => {
    if (movedByTour.current) nextRef.current?.focus();
  }, [index]);

  if (!current || !tips || !place) return null;
  const last = index === tips.length - 1;
  return createPortal(
    <>
      <div className="page-tour-ring" aria-hidden="true" style={place.ring} />
      <div
        ref={cardRef}
        className="page-tour mem-popover-surface"
        role="dialog"
        aria-modal="false"
        aria-label={t("pageTour.label")}
        data-testid="page-tour"
        data-tip={current}
        style={{ top: place.top, left: place.left }}
      >
        <p className="page-tour-text" aria-live="polite">{t(`pageTour.tips.${current}`)}</p>
        <div className="page-tour-actions">
          {tips.length > 1 ? (
            <span className="page-tour-count">{t("pageTour.count", { n: index + 1, total: tips.length })}</span>
          ) : null}
          <button type="button" className="page-tour-close" onClick={finish}>
            {t("pageTour.close")}
          </button>
          <button
            ref={nextRef}
            type="button"
            className="page-tour-next"
            data-testid="page-tour-next"
            onClick={() => {
              if (last) {
                finish();
                return;
              }
              movedByTour.current = true;
              setIndex(index + 1);
            }}
          >
            {t(last ? "pageTour.done" : "pageTour.next")}
          </button>
        </div>
      </div>
    </>,
    document.body,
  );
}
