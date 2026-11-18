import { useState } from "react";
import { House, ListChecks, PuzzlePiece, Stack, X } from "@phosphor-icons/react";
import { TopicIcon } from "../entities/TopicIcon";
import type { GlobalNavigation } from "./viewState";
import { SpaceMark } from "./SpaceMark";
import { NAVIGATION_DESTINATION_ORDER } from "./navigationPreferences";
import type { PinnedPage } from "./pinnedPages";
import type { PageSection } from "../pages/pageSections";

/** A page the person opened lately, with the section it belongs to. */
export type RecentNavigationPage = { readonly id: string; readonly title: string; readonly section: PageSection };
/** How many recently opened pages the left side lists. */
export const SIDEBAR_RECENT_LISTED = 4;

type PrimaryNavigationLabels = {
  readonly entities: string;
  readonly graph: string;
  readonly home: string;
  readonly navigation: string;
  readonly pages: string;
  readonly skills: string;
  readonly rules: string;
  readonly spaces: string;
  /** Heads the pinned pages. */
  readonly pinned?: string;
  /** Heads the recently opened pages. */
  readonly recent?: string;
  /** "Docs, 24 pages", for assistive tech; the row shows only the number. */
  readonly counted?: (label: string, count: number) => string;
  /** "Unpin {{title}}", with the title already filled in by the caller. */
  readonly unpin?: (title: string) => string;
};

type PrimaryNavigationProps = {
  readonly active: GlobalNavigation | null;
  readonly compact?: boolean;
  readonly labels: PrimaryNavigationLabels;
  readonly onNavigateEntities?: () => void;
  readonly onNavigateGraph?: () => void;
  readonly onNavigateHome?: () => void;
  readonly onNavigatePages?: () => void;
  readonly onNavigateSkills?: () => void;
  readonly onNavigateRules?: () => void;
  readonly onNavigateSpaces: (create: boolean) => void;
  /** How many pages each section holds; a section still loading has none. */
  readonly counts?: Partial<Record<PageSection, number>>;
  /** Pages opened lately, newest first, across the three sections. */
  readonly recentPages?: readonly RecentNavigationPage[];
  /** Pages the person pinned, in their order (UX-FIND-6). */
  readonly pinnedPages?: readonly PinnedPage[];
  readonly activePinnedPageId?: string | null;
  readonly onOpenPinnedPage?: (id: string) => void;
  readonly onUnpinPage?: (id: string) => void;
  readonly onMovePinnedPage?: (id: string, beforeId: string | null) => void;
};

type NavButtonProps = {
  readonly active: boolean;
  readonly compact?: boolean;
  readonly icon: React.ReactNode;
  readonly label: string;
  readonly count?: number;
  readonly countedLabel?: string;
  readonly onClick?: () => void;
};

function NavButton({ active, compact = false, icon, label, count, countedLabel, onClick }: NavButtonProps) {
  if (!onClick) return null;
  return (
    <button
      aria-current={active ? "page" : undefined}
      aria-label={label}
      aria-description={count === undefined ? undefined : countedLabel}
      className={compact ? "notes-rail-button" : "notes-primary-link"}
      data-active={active ? "true" : undefined}
      onClick={onClick}
      title={compact ? label : undefined}
      type="button"
    >
      <span aria-hidden="true" className="notes-navigation-glyph">{icon}</span>
      {!compact && <span>{label}</span>}
      {!compact && count !== undefined && <span aria-hidden="true" className="notes-primary-count">{count}</span>}
    </button>
  );
}

const iconStyle = { color: "currentColor" } as const;
const homeIcon = <House aria-hidden="true" data-navigation-icon="home" focusable="false" size={16} style={iconStyle} weight="regular" />;
const pageIcon = <Stack aria-hidden="true" data-navigation-icon="wiki-stack" focusable="false" size={16} style={iconStyle} weight="regular" />;
const skillIcon = <PuzzlePiece aria-hidden="true" data-navigation-icon="skills" focusable="false" size={16} style={iconStyle} weight="regular" />;
const ruleIcon = <ListChecks aria-hidden="true" data-navigation-icon="rules" focusable="false" size={16} style={iconStyle} weight="regular" />;
const entityIcon = <TopicIcon data-navigation-icon="entities" size={16} style={iconStyle} />;
const graphIcon = <svg aria-hidden="true" data-navigation-icon="graph" height="16" style={iconStyle} viewBox="0 0 24 24" width="16"><circle cx="5" cy="6" fill="none" r="2" stroke="currentColor" strokeWidth="2" /><circle cx="19" cy="6" fill="none" r="2" stroke="currentColor" strokeWidth="2" /><circle cx="12" cy="18" fill="none" r="2" stroke="currentColor" strokeWidth="2" /><path d="M7 6h10M6 8l5 8M18 8l-5 8" fill="none" stroke="currentColor" strokeWidth="2" /></svg>;

export function PrimaryNavigation({
  active,
  compact = false,
  labels,
  onNavigateEntities,
  onNavigateGraph,
  onNavigateHome,
  onNavigatePages,
  onNavigateSkills,
  onNavigateRules,
  onNavigateSpaces,
  counts = {},
  recentPages = [],
  pinnedPages = [],
  activePinnedPageId = null,
  onOpenPinnedPage,
  onUnpinPage,
  onMovePinnedPage,
}: PrimaryNavigationProps) {
  const [draggedPin, setDraggedPin] = useState<string | null>(null);
  const options = {
    home: { icon: homeIcon, onClick: onNavigateHome, count: undefined },
    pages: { icon: pageIcon, onClick: onNavigatePages, count: counts.docs },
    skills: { icon: skillIcon, onClick: onNavigateSkills, count: counts.skills },
    rules: { icon: ruleIcon, onClick: onNavigateRules, count: counts.rules },
    spaces: { icon: <SpaceMark active={active === "spaces"} inheritColor />, onClick: () => onNavigateSpaces(false), count: undefined },
    graph: { icon: graphIcon, onClick: onNavigateGraph, count: undefined },
    entities: { icon: entityIcon, onClick: onNavigateEntities, count: undefined },
  };
  // The same rows in the same order, always: nothing is nested under a row
  // and nothing folds (UX-FIND-4). A section opens its list in the main
  // area. Graph appears only where it can open (not for a team member on
  // the web); topics have no row.
  const destinations = NAVIGATION_DESTINATION_ORDER
    .filter((key) => key !== "entities")
    .map((key) => ({ key, label: labels[key], ...options[key] }));
  const recent = compact ? [] : recentPages.slice(0, SIDEBAR_RECENT_LISTED);
  const sectionLabel: Record<PageSection, string> = { docs: labels.pages, skills: labels.skills, rules: labels.rules };

  return (
    <nav aria-label={labels.navigation} className={`notes-rail-nav${compact ? " notes-rail-nav--icons" : " notes-rail-nav--labels"}`}>
      {destinations.map((destination) => (
        <NavButton active={active === destination.key} compact={compact} count={destination.count} countedLabel={destination.count === undefined ? undefined : labels.counted?.(destination.label, destination.count)} icon={destination.icon} key={destination.key} label={destination.label} onClick={destination.onClick} />
      ))}
      {onOpenPinnedPage && pinnedPages.length > 0 && (
        <div aria-label={labels.pinned} className="notes-pinned-pages" role="group">
          {!compact && labels.pinned && <p aria-hidden="true" className="notes-rail-heading">{labels.pinned}</p>}
          {pinnedPages.map((page) => (
            <div
              className="notes-pinned-page"
              data-dragging={draggedPin === page.id ? "true" : undefined}
              draggable={!compact && !!onMovePinnedPage}
              key={page.id}
              onDragEnd={() => setDraggedPin(null)}
              onDragOver={(event) => { if (draggedPin && draggedPin !== page.id) event.preventDefault(); }}
              onDragStart={(event) => { setDraggedPin(page.id); event.dataTransfer.effectAllowed = "move"; }}
              onDrop={(event) => {
                event.preventDefault();
                if (draggedPin) onMovePinnedPage?.(draggedPin, page.id);
                setDraggedPin(null);
              }}
            >
              <button
                aria-current={activePinnedPageId === page.id ? "page" : undefined}
                aria-label={page.title}
                className={compact ? "notes-rail-button" : "notes-primary-link"}
                data-active={activePinnedPageId === page.id ? "true" : undefined}
                onClick={() => onOpenPinnedPage(page.id)}
                title={compact ? page.title : undefined}
                type="button"
              >
                {compact && (
                  <span aria-hidden="true" className="notes-navigation-glyph">
                    <span className="notes-pinned-page-initial">{[...page.title.trim()][0] ?? ""}</span>
                  </span>
                )}
                {!compact && <span className="notes-pinned-page-title">{page.title}</span>}
              </button>
              {!compact && onUnpinPage && labels.unpin && (
                <button
                  aria-label={labels.unpin(page.title)}
                  className="notes-pinned-page-unpin"
                  onClick={() => onUnpinPage(page.id)}
                  title={labels.unpin(page.title)}
                  type="button"
                >
                  <X aria-hidden="true" size={12} />
                </button>
              )}
            </div>
          ))}
        </div>
      )}
      {onOpenPinnedPage && recent.length > 0 && (
        <div aria-label={labels.recent} className="notes-recent-pages" role="group">
          {labels.recent && <p aria-hidden="true" className="notes-rail-heading">{labels.recent}</p>}
          {recent.map((page) => (
            <button
              aria-current={activePinnedPageId === page.id ? "page" : undefined}
              aria-label={page.title}
              aria-description={sectionLabel[page.section]}
              className="notes-primary-link notes-recent-page"
              data-active={activePinnedPageId === page.id ? "true" : undefined}
              data-section={page.section}
              key={page.id}
              onClick={() => onOpenPinnedPage(page.id)}
              type="button"
            >
              <span className="notes-pinned-page-title">{page.title}</span>
              <span aria-hidden="true" className="notes-recent-page-kind">{sectionLabel[page.section]}</span>
            </button>
          ))}
        </div>
      )}
    </nav>
  );
}
