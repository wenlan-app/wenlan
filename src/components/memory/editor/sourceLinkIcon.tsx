// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { createRoot } from "react-dom/client";
import { SourceTypeIcon } from "../sources/SourceTypeIcon";
import { getSourceAppFromUrl } from "../sources/sourcePresentation";
import type { SourceCardMount, SourceIconMount, SourceLinkPlace } from "./sourcePaste";
import "./sourceLinkIcon.css";

export interface SourceLinkIconActions {
  /** Says what the icon means, such as "Saved in Sources". */
  label: string;
  open(url: string): void;
  /** The menu's words; without them the icon only opens the source. */
  menu?: { showLink: string; showPreview: string; openSource: string; more: string };
  /** The first words of a saved source, or null when only its link was saved. */
  excerpt?(url: string): Promise<string | null>;
}

const excerpts = new Map<string, Promise<string | null>>();

/** A source's first words, asked once per link. */
function useExcerpt(url: string, actions: SourceLinkIconActions | undefined, wanted: boolean): string | null | undefined {
  const [excerpt, setExcerpt] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    if (!wanted || !actions?.excerpt) return;
    let live = true;
    let pending = excerpts.get(url);
    if (!pending) {
      pending = actions.excerpt(url).catch(() => null);
      excerpts.set(url, pending);
      // A source saved later can have words; ask again next time.
      void pending.then((text) => { if (text === null) excerpts.delete(url); });
    }
    void pending.then((text) => { if (live) setExcerpt(text); });
    return () => { live = false; };
  }, [url, actions, wanted]);
  return actions?.excerpt ? excerpt : null;
}

function SourceLinkMenu({ anchor, url, place, actions, onClose, canPreview }: {
  anchor: HTMLElement;
  url: string;
  place: SourceLinkPlace;
  actions: SourceLinkIconActions;
  onClose(): void;
  canPreview: boolean;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  useLayoutEffect(() => {
    const rect = anchor.getBoundingClientRect();
    const width = menuRef.current?.offsetWidth ?? 200;
    setPosition({ top: rect.bottom + 4, left: Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)) });
  }, [anchor]);
  useEffect(() => {
    const closeOutside = (event: Event) => {
      if (!(event.target instanceof Node) || (!menuRef.current?.contains(event.target) && !anchor.contains(event.target))) onClose();
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    document.addEventListener("pointerdown", closeOutside, true);
    document.addEventListener("keydown", closeOnEscape, true);
    window.addEventListener("resize", onClose);
    return () => {
      document.removeEventListener("pointerdown", closeOutside, true);
      document.removeEventListener("keydown", closeOnEscape, true);
      window.removeEventListener("resize", onClose);
    };
  }, [anchor, onClose]);
  const words = actions.menu!;
  const items = [
    ...(canPreview ? [
      { key: "link", label: words.showLink, checked: !place.preview, select: () => place.setPreview(false) },
      { key: "preview", label: words.showPreview, checked: place.preview, select: () => place.setPreview(true) },
    ] : []),
    { key: "open", label: words.openSource, checked: false, select: () => actions.open(url) },
  ];
  return createPortal(
    <div
      ref={menuRef}
      className="mem-popover-surface source-link-menu"
      role="menu"
      aria-label={actions.label}
      style={position ? { top: position.top, left: position.left } : { visibility: "hidden" }}
    >
      <span className="source-link-menu__title" aria-hidden="true">{actions.label}</span>
      {items.map((item) => <button
        key={item.key}
        type="button"
        role={item.key === "open" ? "menuitem" : "menuitemradio"}
        aria-checked={item.key === "open" ? undefined : item.checked}
        className="source-link-menu__item"
        // Keep the caret where it is.
        onMouseDown={(event) => event.preventDefault()}
        onClick={(event) => {
          event.preventDefault();
          onClose();
          item.select();
        }}
      >
        <span>{item.label}</span>
        {item.checked && <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5" /></svg>}
      </button>)}
    </div>,
    document.body,
  );
}

function SourceLinkIcon({ url, place, actions }: { url: string; place?: SourceLinkPlace; actions?: () => SourceLinkIconActions }) {
  const current = actions?.();
  const buttonRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const offersPreview = Boolean(place?.canPreview && current?.menu);
  const excerpt = useExcerpt(url, current, offersPreview);
  const icon = <SourceTypeIcon kind={getSourceAppFromUrl(url) ?? "webpage"} size={14} />;
  if (!current) return <span aria-hidden="true">{icon}</span>;
  // A card is offered only for a source with words to show.
  const canPreview = offersPreview && (Boolean(excerpt) || Boolean(place?.preview));
  return <>
    <button
      ref={buttonRef}
      type="button"
      title={current.label}
      aria-label={current.label}
      aria-haspopup={canPreview ? "menu" : undefined}
      aria-expanded={canPreview ? open : undefined}
      // Keep the caret where it is: the click is not a move into the link.
      onMouseDown={(event) => event.preventDefault()}
      onClick={(event) => {
        event.preventDefault();
        if (canPreview && place) setOpen((value) => !value);
        else actions?.().open(url);
      }}
    >
      {icon}
    </button>
    {open && place && buttonRef.current && <SourceLinkMenu
      anchor={buttonRef.current}
      url={url}
      place={place}
      actions={actions!()}
      canPreview={canPreview}
      onClose={() => setOpen(false)}
    />}
  </>;
}

function SourceLinkCard({ url, title, place, actions }: { url: string; title: string; place: SourceLinkPlace; actions: () => SourceLinkIconActions }) {
  const current = actions();
  const moreRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const excerpt = useExcerpt(url, current, true);
  let site = url;
  try {
    site = new URL(url).hostname.replace(/^www\./, "");
  } catch {
    // Shown as written.
  }
  return <span className="source-link-card">
    <button
      type="button"
      className="source-link-card__body"
      title={current.label}
      onMouseDown={(event) => event.preventDefault()}
      onClick={(event) => {
        event.preventDefault();
        actions().open(url);
      }}
    >
      <span className="source-link-card__title">
        <SourceTypeIcon kind={getSourceAppFromUrl(url) ?? "webpage"} size={16} />
        <span>{title}</span>
      </span>
      <span className="source-link-card__site">{site}</span>
      {/* A short post is its own title; say it once. */}
      {excerpt && excerpt !== title.replace(/\s+/g, " ").trim() && <span className="source-link-card__excerpt">{excerpt}</span>}
    </button>
    {current.menu && <button
      ref={moreRef}
      type="button"
      className="mem-icon-action source-link-card__more"
      aria-label={current.menu.more}
      aria-haspopup="menu"
      aria-expanded={open}
      onMouseDown={(event) => event.preventDefault()}
      onClick={(event) => {
        event.preventDefault();
        setOpen((value) => !value);
      }}
    >
      <svg aria-hidden="true" width="14" height="4" viewBox="0 0 16 4" fill="currentColor">
        <circle cx="2" cy="2" r="1.5" /><circle cx="8" cy="2" r="1.5" /><circle cx="14" cy="2" r="1.5" />
      </svg>
    </button>}
    {open && moreRef.current && <SourceLinkMenu
      anchor={moreRef.current}
      url={url}
      place={place}
      actions={current}
      canPreview={place.canPreview}
      onClose={() => setOpen(false)}
    />}
  </span>;
}

/** The editor's site icons are the same ones Sources shows; a click opens the
 *  source, or, for a link alone on its line, a menu to show it as a card. */
export function sourceLinkIconMount(actions?: () => SourceLinkIconActions): SourceIconMount {
  return (container, url, place) => {
    const root = createRoot(container);
    root.render(<SourceLinkIcon url={url} place={place} actions={actions} />);
    // CodeMirror can drop a widget while React is rendering; unmount after it.
    return () => queueMicrotask(() => root.unmount());
  };
}

/** A saved link shown as a card: its title, site and first words. */
export function sourceLinkCardMount(actions: () => SourceLinkIconActions): SourceCardMount {
  return (container, url, title, place) => {
    const root = createRoot(container);
    root.render(<SourceLinkCard url={url} title={title} place={place} actions={actions} />);
    return () => queueMicrotask(() => root.unmount());
  };
}

export const mountSourceLinkIcon: SourceIconMount = sourceLinkIconMount();
