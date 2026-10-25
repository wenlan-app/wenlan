// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { createRoot } from "react-dom/client";
import { SourceTypeIcon } from "../sources/SourceTypeIcon";
import { getSourceAppFromUrl } from "../sources/sourcePresentation";
import { postEmbedHeight, postEmbedOrigin, postEmbedUrl, sourceEmbed, videoCoverUrl, videoPlayerUrl, type SourceEmbed } from "./sourceEmbed";
import type { SourceCardMount, SourceIconMount, SourceLinkPlace } from "./sourcePaste";
import "./sourceLinkIcon.css";

export interface SourceLinkIconActions {
  /** Says what the icon means, such as "Saved in Sources". */
  label: string;
  open(url: string): void;
  /** The menu's words; without them the icon only opens the source. */
  menu?: { showLink: string; showPreview: string; openSource: string; more: string; play?: string };
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
  const [spot, setSpot] = useState<{ top: number; left: number } | null>(null);
  useLayoutEffect(() => {
    const rect = anchor.getBoundingClientRect();
    const width = menuRef.current?.offsetWidth ?? 200;
    const height = menuRef.current?.offsetHeight ?? 0;
    // Opens upward when there is no room below, as for a card at the bottom of the window.
    const below = rect.bottom + 4;
    const top = below + height > window.innerHeight - 8 ? Math.max(8, rect.top - height - 4) : below;
    setSpot({ top, left: Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)) });
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
      style={spot ? { top: spot.top, left: spot.left } : { visibility: "hidden" }}
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

/** Whether a card's text only says its title again, as a short post does,
 *  or a post whose title is its own words cut short. */
function repeatsTitle(excerpt: string, title: string): boolean {
  const stem = title.replace(/\s+/g, " ").trim().replace(/…$/, "").trimEnd();
  return excerpt === stem || (title.trim().endsWith("…") && stem.length >= 10 && excerpt.startsWith(stem));
}

type EmbedState = "waiting" | "shown" | "failed";

/** How long a post may take to show before its card stays words only. */
const POST_EMBED_WAIT_MS = 15_000;

/** A post shown by its site's own embed page. It stays out of sight until the
 *  page says how tall the post is, which it does only once the post is drawn. */
function PostEmbed({ embed, title, onState, resized }: {
  embed: Extract<SourceEmbed, { kind: "post" }>;
  title: string;
  onState(state: EmbedState): void;
  resized?(): void;
}) {
  const frameRef = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(0);
  const [dark] = useState(() => document.documentElement.getAttribute("data-theme") !== "light");
  const src = useMemo(() => postEmbedUrl(embed, dark), [embed, dark]);
  const shown = height > 80;
  useEffect(() => {
    const listen = (event: MessageEvent) => {
      if (event.source !== frameRef.current?.contentWindow || !postEmbedOrigin(embed, event.origin)) return;
      if (import.meta.env.DEV) console.debug("[source embed]", event.origin, event.data);
      const asked = postEmbedHeight(event.data);
      if (asked !== null) setHeight(Math.min(Math.max(Math.round(asked), 0), 2400));
    };
    window.addEventListener("message", listen);
    return () => window.removeEventListener("message", listen);
  }, [embed]);
  useEffect(() => {
    if (shown) {
      onState("shown");
      return;
    }
    // Offline, or a site that would not draw the post.
    const timer = window.setTimeout(() => onState("failed"), POST_EMBED_WAIT_MS);
    return () => window.clearTimeout(timer);
  }, [shown, onState]);
  useEffect(() => { resized?.(); }, [height, resized]);
  return <iframe
    ref={frameRef}
    className={`source-link-card__post${shown ? "" : " source-link-card__post--waiting"}`}
    title={title}
    src={src}
    // The frame holds the site's own page at the site's own address, so it
    // keeps its origin; it may not move the app or open dialogs over it.
    sandbox="allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox"
    referrerPolicy="strict-origin-when-cross-origin"
    style={shown ? { height } : undefined}
    aria-hidden={shown ? undefined : true}
    tabIndex={shown ? undefined : -1}
  />;
}

/** A video's cover, and YouTube's own player in its place once played. */
function VideoCover({ id, title, play, onState }: { id: string; title: string; play: string; onState(state: EmbedState): void }) {
  const [playing, setPlaying] = useState(false);
  if (playing) {
    return <iframe
      className="source-link-card__video"
      src={videoPlayerUrl(id)}
      title={title}
      allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
      allowFullScreen
      referrerPolicy="strict-origin-when-cross-origin"
    />;
  }
  return <button
    type="button"
    className="source-link-card__video source-link-card__cover"
    aria-label={`${play}: ${title}`}
    onMouseDown={(event) => event.preventDefault()}
    onClick={(event) => {
      event.preventDefault();
      setPlaying(true);
    }}
  >
    <img src={videoCoverUrl(id)} alt="" draggable={false} onLoad={() => onState("shown")} onError={() => onState("failed")} />
    <span className="source-link-card__play" aria-hidden="true">
      <svg width="26" height="26" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5.5v13l10.5-6.5z" /></svg>
    </span>
  </button>;
}

function SourceLinkCard({ url, title, place, actions }: { url: string; title: string; place: SourceLinkPlace; actions: () => SourceLinkIconActions }) {
  const current = actions();
  const moreRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const embed = useMemo(() => sourceEmbed(url), [url]);
  const [embedState, setEmbedState] = useState<EmbedState>("waiting");
  const excerpt = useExcerpt(url, current, true);
  // A post named by its own words reads whole in the card, said once.
  const repeated = Boolean(excerpt && repeatsTitle(excerpt, title));
  // The site's own post, or the video, says the words; the card only names it.
  const media = embed && embedState === "shown";
  const heading = repeated && !media ? excerpt! : title;
  let site = url;
  try {
    site = new URL(url).hostname.replace(/^www\./, "");
  } catch {
    // Shown as written.
  }
  // Offline, or a site that would not draw the post: the card stays words only.
  const showEmbed = embed && embedState !== "failed";
  const resized = place.resized;
  useEffect(() => { resized?.(); }, [embedState, resized]);
  return <span className={`source-link-card${media ? " source-link-card--media" : ""}`}>
    {showEmbed && embed.kind === "video" && <VideoCover id={embed.id} title={title} play={current.menu?.play ?? title} onState={setEmbedState} />}
    <span className="source-link-card__head">
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
        <span className={`source-link-card__title${repeated && !media ? " source-link-card__title--post" : ""}`}>
          <SourceTypeIcon kind={getSourceAppFromUrl(url) ?? "webpage"} size={16} />
          <span>{heading}</span>
        </span>
        <span className="source-link-card__site">{site}</span>
        {excerpt && !repeated && !media && <span className="source-link-card__excerpt">{excerpt}</span>}
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
    </span>
    {showEmbed && embed.kind === "post" && <PostEmbed embed={embed} title={title} onState={setEmbedState} resized={resized} />}
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
