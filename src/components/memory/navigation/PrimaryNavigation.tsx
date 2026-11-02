import { House, Stack } from "@phosphor-icons/react";
import { TopicIcon } from "../entities/TopicIcon";
import type { GlobalNavigation } from "./viewState";
import { SpaceMark } from "./SpaceMark";
import { NAVIGATION_DESTINATION_ORDER } from "./navigationPreferences";

type PrimaryNavigationLabels = {
  readonly entities: string;
  readonly graph: string;
  readonly home: string;
  readonly navigation: string;
  readonly pages: string;
  readonly spaces: string;
};

type PrimaryNavigationProps = {
  readonly active: GlobalNavigation | null;
  readonly compact?: boolean;
  readonly labels: PrimaryNavigationLabels;
  readonly onNavigateEntities?: () => void;
  readonly onNavigateGraph?: () => void;
  readonly onNavigateHome?: () => void;
  readonly onNavigatePages?: () => void;
  readonly onNavigateSpaces: (create: boolean) => void;
};

type NavButtonProps = {
  readonly active: boolean;
  readonly compact?: boolean;
  readonly icon: React.ReactNode;
  readonly label: string;
  readonly onClick?: () => void;
};

function NavButton({ active, compact = false, icon, label, onClick }: NavButtonProps) {
  if (!onClick) return null;
  return (
    <button
      aria-current={active ? "page" : undefined}
      aria-label={label}
      className={compact ? "notes-rail-button" : "notes-primary-link"}
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

export function PrimaryNavigation({
  active,
  compact = false,
  labels,
  onNavigateEntities,
  onNavigateGraph,
  onNavigateHome,
  onNavigatePages,
  onNavigateSpaces,
}: PrimaryNavigationProps) {
  const options = {
    home: { icon: homeIcon, onClick: onNavigateHome },
    pages: { icon: pageIcon, onClick: onNavigatePages },
    spaces: { icon: <SpaceMark active={active === "spaces"} inheritColor />, onClick: () => onNavigateSpaces(false) },
    graph: { icon: graphIcon, onClick: onNavigateGraph },
    entities: { icon: entityIcon, onClick: onNavigateEntities },
  };
  // Every destination sits on the left: there are few enough that a More
  // menu only hid them (UX-FIND-4).
  const destinations = NAVIGATION_DESTINATION_ORDER.map((key) => ({ key, label: labels[key], ...options[key] }));

  return (
    <nav aria-label={labels.navigation} className={`notes-rail-nav${compact ? " notes-rail-nav--icons" : " notes-rail-nav--labels"}`}>
      {destinations.map((destination) => (
        <NavButton active={active === destination.key} compact={compact} icon={destination.icon} key={destination.key} label={destination.label} onClick={destination.onClick} />
      ))}
    </nav>
  );
}
