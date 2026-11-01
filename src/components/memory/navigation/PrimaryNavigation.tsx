import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Files, House, PushPin, Stack } from "@phosphor-icons/react";
import { TopicIcon } from "../entities/TopicIcon";
import type { GlobalNavigation } from "./viewState";
import { SpaceMark } from "./SpaceMark";
import { NAVIGATION_DESTINATION_ORDER, readNavigationPreferences, REQUIRED_NAVIGATION_DESTINATIONS, sanitizeVisibleNavigation, writeNavigationPreferences } from "./navigationPreferences";

type PrimaryNavigationLabels = {
  readonly entities: string;
  readonly graph: string;
  readonly home: string;
  readonly memories: string;
  readonly more: string;
  readonly navigation: string;
  readonly pages: string;
  readonly pinToSidebar: (name: string) => string;
  readonly sources: string;
  readonly spaces: string;
  readonly unpinFromSidebar: (name: string) => string;
};

type PrimaryNavigationProps = {
  readonly active: GlobalNavigation | null;
  readonly compact?: boolean;
  readonly labels: PrimaryNavigationLabels;
  readonly onNavigateEntities?: () => void;
  readonly onNavigateGraph?: () => void;
  readonly onNavigateHome?: () => void;
  readonly onNavigateLog?: () => void;
  readonly onNavigatePages?: () => void;
  readonly onNavigateSources?: () => void;
  readonly onNavigateSpaces: (create: boolean) => void;
};

type NavButtonProps = {
  readonly active: boolean;
  readonly compact?: boolean;
  readonly icon: React.ReactNode;
  readonly label: string;
  readonly moreItem?: boolean;
  readonly onClick?: () => void;
};

function NavButton({ active, compact = false, icon, label, moreItem = false, onClick }: NavButtonProps) {
  if (!onClick) return null;
  return (
    <button
      aria-current={active ? "page" : undefined}
      aria-label={label}
      className={moreItem ? "notes-more-destination-link" : compact ? "notes-rail-button" : "notes-primary-link"}
      data-active={active ? "true" : undefined}
      onClick={onClick}
      title={compact ? label : undefined}
      type="button"
    >
      <span aria-hidden="true" className="notes-navigation-glyph">{icon}</span>
      {!compact && <span>{label}</span>}
    </button>
  );
}

const iconStyle = { color: "currentColor" } as const;
const homeIcon = <House aria-hidden="true" data-navigation-icon="home" focusable="false" size={16} style={iconStyle} weight="regular" />;
const pageIcon = <Stack aria-hidden="true" data-navigation-icon="wiki-stack" focusable="false" size={16} style={iconStyle} weight="regular" />;
const entityIcon = <TopicIcon data-navigation-icon="entities" size={16} style={iconStyle} />;
const graphIcon = <svg aria-hidden="true" data-navigation-icon="graph" height="16" style={iconStyle} viewBox="0 0 24 24" width="16"><circle cx="5" cy="6" fill="none" r="2" stroke="currentColor" strokeWidth="2" /><circle cx="19" cy="6" fill="none" r="2" stroke="currentColor" strokeWidth="2" /><circle cx="12" cy="18" fill="none" r="2" stroke="currentColor" strokeWidth="2" /><path d="M7 6h10M6 8l5 8M18 8l-5 8" fill="none" stroke="currentColor" strokeWidth="2" /></svg>;
const memoryIcon = <svg aria-hidden="true" data-navigation-icon="brain" fill="none" height="16" style={iconStyle} viewBox="0 0 24 24" width="16">{[
  "M15.5 13a3.5 3.5 0 0 0 -3.5 3.5v1a3.5 3.5 0 0 0 7 0v-1.8",
  "M8.5 13a3.5 3.5 0 0 1 3.5 3.5v1a3.5 3.5 0 0 1 -7 0v-1.8",
  "M17.5 16a3.5 3.5 0 0 0 0 -7h-.5",
  "M19 9.3v-2.8a3.5 3.5 0 0 0 -7 0",
  "M6.5 16a3.5 3.5 0 0 1 0 -7h.5",
  "M5 9.3v-2.8a3.5 3.5 0 0 1 7 0v10",
].map((d) => <path d={d} key={d} stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.8" />)}</svg>;
const sourceIcon = <Files aria-hidden="true" data-navigation-icon="sources-library" focusable="false" size={16} style={iconStyle} weight="regular" />;

export function PrimaryNavigation({
  active,
  compact = false,
  labels,
  onNavigateEntities,
  onNavigateGraph,
  onNavigateHome,
  onNavigateLog,
  onNavigatePages,
  onNavigateSources,
  onNavigateSpaces,
}: PrimaryNavigationProps) {
  const [visibleState, setVisibleState] = useState(readNavigationPreferences);
  const visible = sanitizeVisibleNavigation(visibleState);
  const [moreOpen, setMoreOpen] = useState(false);
  const [panelPosition, setPanelPosition] = useState({ left: 51, top: 0, maxHeight: 0 });
  const moreRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const moreButtonRef = useRef<HTMLButtonElement>(null);
  const options = {
    home: { icon: homeIcon, onClick: onNavigateHome },
    pages: { icon: pageIcon, onClick: onNavigatePages },
    spaces: { icon: <SpaceMark active={active === "spaces"} inheritColor />, onClick: () => onNavigateSpaces(false) },
    graph: { icon: graphIcon, onClick: onNavigateGraph },
    sources: { icon: sourceIcon, onClick: onNavigateSources },
    memories: { icon: memoryIcon, onClick: onNavigateLog },
    entities: { icon: entityIcon, onClick: onNavigateEntities },
  };
  const destinations = NAVIGATION_DESTINATION_ORDER.map((key) => ({ key, label: labels[key], ...options[key] })).filter((destination) => destination.onClick !== undefined);
  const optionalDestinations = destinations.filter((destination) => !REQUIRED_NAVIGATION_DESTINATIONS.includes(destination.key));
  const railDestinations = destinations.filter((destination) => visible.includes(destination.key));
  const moreActive = destinations.some((destination) => destination.key === active && !visible.includes(destination.key));
  const moreMarked = moreActive && !moreOpen;

  const closeMore = () => setMoreOpen(false);

  useEffect(() => {
    if (!moreOpen) return;
    const closeOutside = (event: PointerEvent) => {
      if (!moreRef.current?.contains(event.target as Node)) {
        setMoreOpen(false);
      }
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => document.removeEventListener("pointerdown", closeOutside);
  }, [moreOpen]);

  useLayoutEffect(() => {
    if (!moreOpen) return;
    const positionPanel = () => {
      const anchor = moreRef.current?.getBoundingClientRect();
      const panel = panelRef.current?.getBoundingClientRect();
      if (!anchor || !panel) return;
      const margin = 8;
      const maxHeight = Math.max(0, window.innerHeight - margin * 2);
      const left = Math.max(margin, Math.min(anchor.right + 7, window.innerWidth - panel.width - margin));
      const top = Math.max(margin, Math.min(anchor.top, window.innerHeight - Math.min(panel.height, maxHeight) - margin));
      setPanelPosition((current) => current.left === left && current.top === top && current.maxHeight === maxHeight
        ? current : { left, top, maxHeight });
    };
    positionPanel();
    window.addEventListener("resize", positionPanel);
    window.addEventListener("scroll", positionPanel, true);
    return () => {
      window.removeEventListener("resize", positionPanel);
      window.removeEventListener("scroll", positionPanel, true);
    };
  }, [moreOpen]);

  const updateVisible = (next: readonly GlobalNavigation[]) => {
    const sanitized = sanitizeVisibleNavigation(next);
    setVisibleState(sanitized);
    writeNavigationPreferences(sanitized);
  };
  const navigate = (action: (() => void) | undefined) => action && (() => {
    closeMore();
    action();
  });

  return (
    <nav aria-label={labels.navigation} className={`notes-rail-nav${compact ? " notes-rail-nav--icons" : " notes-rail-nav--labels"}`}>
      {railDestinations.map((destination) => (
        <NavButton active={active === destination.key} compact={compact} icon={destination.icon} key={destination.key} label={destination.label} onClick={destination.onClick} />
      ))}
      <div
        className="notes-more-anchor"
        data-sidebar-escape-scope={moreOpen ? "true" : undefined}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          event.stopPropagation();
          closeMore();
          moreButtonRef.current?.focus();
        }}
        ref={moreRef}
      >
        <button
          aria-controls={moreOpen ? "notes-more-destinations" : undefined}
          aria-current={moreMarked ? "page" : undefined}
          aria-expanded={moreOpen}
          aria-label={labels.more}
          className={compact ? "notes-rail-button" : "notes-primary-link"}
          data-active={moreMarked ? "true" : undefined}
          data-sidebar-more="true"
          onClick={(event) => {
            event.currentTarget.focus({ preventScroll: true });
            setMoreOpen((current) => !current);
          }}
          ref={moreButtonRef}
          title={labels.more}
          type="button"
        >
          <span aria-hidden="true" className="notes-navigation-glyph"><svg aria-hidden="true" fill="currentColor" height="16" viewBox="0 0 24 24" width="16"><circle cx="5" cy="12" r="1.5" /><circle cx="12" cy="12" r="1.5" /><circle cx="19" cy="12" r="1.5" /></svg></span>
          {!compact && <span>{labels.more}</span>}
        </button>
        {moreOpen && (
          <div
            aria-label={labels.more}
            className="notes-more-panel"
            id="notes-more-destinations"
            ref={panelRef}
            role="group"
            style={{ left: panelPosition.left, top: panelPosition.top, maxHeight: panelPosition.maxHeight || undefined }}
          >
            {optionalDestinations.map((destination) => {
              const pinned = visible.includes(destination.key);
              const pinLabel = pinned
                ? labels.unpinFromSidebar(destination.label)
                : labels.pinToSidebar(destination.label);
              const activeInMore = active === destination.key && !pinned && moreOpen;
              return (
                <div className="notes-more-destination" data-active={activeInMore ? "true" : undefined} key={destination.key}>
                  <NavButton
                    active={activeInMore}
                    icon={destination.icon}
                    label={destination.label}
                    moreItem
                    onClick={navigate(destination.onClick)}
                  />
                  <button
                    aria-label={pinLabel}
                    aria-pressed={pinned}
                    className="notes-more-pin"
                    data-pinned={pinned ? "true" : "false"}
                    onClick={(event) => {
                      event.currentTarget.focus({ preventScroll: true });
                      updateVisible(pinned ? visible.filter((key) => key !== destination.key) : [...visible, destination.key]);
                    }}
                    title={pinLabel}
                    type="button"
                  >
                    <PushPin aria-hidden="true" size={18} weight={pinned ? "fill" : "regular"} />
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </nav>
  );
}
