// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPageOriginal, getPageSessions, type AgentSession, type PageOriginal } from "../../../../lib/tauri";
import { formatSessionDay, sessionToolLabel } from "../../../../lib/sessionSource";
import { SessionLine } from "../../sessions/SessionSourceSection";
import { SessionCard } from "../../sessions/SessionCard";
import { frameDocument, splitOriginal, type OriginalParts } from "./originalContent";
import "./original.css";

const MIN_FRAME_HEIGHT = 120;

/** The original HTML or SVG, as it was, in a frame that cannot reach the app. */
export function OriginalFrame({ frame, title }: { frame: NonNullable<OriginalParts["frame"]>; title: string }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(480);
  const doc = useMemo(() => frameDocument(frame), [frame]);
  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      if (event.source !== ref.current?.contentWindow) return;
      const value = (event.data as { wenlanOriginalHeight?: unknown } | null)?.wenlanOriginalHeight;
      if (typeof value === "number" && Number.isFinite(value)) {
        setHeight(Math.max(MIN_FRAME_HEIGHT, Math.ceil(value)));
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);
  return (
    <iframe
      ref={ref}
      className="original-frame"
      data-testid="original-frame"
      title={t("originalPage.frameTitle", { title })}
      // Scripts run, as they did where the AI made it, but without the app's
      // origin: the frame cannot read Wenlan, its storage or its commands.
      sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox allow-forms allow-modals"
      referrerPolicy="no-referrer"
      srcDoc={doc}
      style={{ height }}
    />
  );
}

async function openExternal(url: string) {
  const { open } = await import("@tauri-apps/plugin-shell");
  await open(url);
}

/**
 * Where it came from (UX-ART-2): the conversation that kept it, opening the
 * conversation card, or for an imported chat the tool and the day, with a
 * way to open the chat in that tool.
 */
function OriginalSource({ pageId, original, onOpenMemory, onOpenPage }: {
  pageId: string;
  original: PageOriginal | null;
  onOpenMemory?: (sourceId: string) => void;
  onOpenPage?: (pageId: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState<AgentSession | null>(null);
  const sessions = useQuery({
    queryKey: ["page-sessions", pageId],
    queryFn: () => getPageSessions(pageId),
    staleTime: 30_000,
    retry: false,
  });
  const wrote = (sessions.data ?? []).find((entry) => entry.via === "wrote")?.session;
  if (wrote) {
    return (
      <div className="original-source" data-testid="original-source">
        <SessionLine session={wrote} onOpen={() => setOpen(wrote)} />
        {open && (
          <SessionCard sessionId={open.id} known={open} onClose={() => setOpen(null)}
            onOpenMemory={onOpenMemory} onOpenPage={onOpenPage} />
        )}
      </div>
    );
  }
  if (!original) return null;
  const tool = sessionToolLabel(original.vendor);
  const when = original.written_at ?? original.imported_at;
  const text = [t("sessionSource.from", { tool }), formatSessionDay(when, i18n.language)].filter(Boolean).join(" · ");
  return (
    <div className="original-source" data-testid="original-source">
      <span className="original-source-text">{text}</span>
      {original.conversation_url && (
        <button type="button" className="session-source-line" onClick={() => { void openExternal(original.conversation_url); }}>
          {t("originalPage.openIn", { tool })}
        </button>
      )}
    </div>
  );
}

/**
 * Its versions: on an older one, the newest that replaced it; on a newer
 * one, the ones it replaced, newest first. Each opens its own page.
 */
function OriginalVersions({ original, where, onOpenPage }: {
  original: PageOriginal | null;
  where: "newer" | "older";
  onOpenPage?: (pageId: string) => void;
}) {
  const { t, i18n } = useTranslation();
  if (!original) return null;
  if (where === "newer") {
    const newer = original.newer;
    if (!newer) return null;
    return (
      <div className="original-newer" data-testid="original-newer">
        <button type="button" className="session-source-line" onClick={() => onOpenPage?.(newer.page_id)}>
          {t("originalPage.newer", { title: newer.title })}
        </button>
      </div>
    );
  }
  const older = original.older ?? [];
  if (older.length === 0) return null;
  return (
    <div className="original-older" data-testid="original-older">
      <span className="original-older-label">{t("originalPage.older")}</span>
      {older.map((version) => (
        <button key={version.page_id} type="button" className="session-source-line"
          onClick={() => onOpenPage?.(version.page_id)}>
          {[version.title, formatSessionDay(version.kept_at, i18n.language)].filter(Boolean).join(" · ")}
        </button>
      ))}
    </div>
  );
}

/**
 * A page that keeps what an AI tool made: where it came from, then the
 * original exactly as it was (UX-ART-1), then any notes added after it
 * (UX-ART-3: the AI never rewrites the original, it only adds notes).
 */
export function OriginalPageView({ pageId, title, content, renderMarkdown, onOpenMemory, onOpenPage }: {
  pageId: string;
  title: string;
  content: string;
  renderMarkdown: (markdown: string) => ReactNode;
  onOpenMemory?: (sourceId: string) => void;
  onOpenPage?: (pageId: string) => void;
}) {
  const parts = useMemo(() => splitOriginal(content), [content]);
  const original = useQuery({
    queryKey: ["page-original", pageId],
    queryFn: () => getPageOriginal(pageId),
    staleTime: 60_000,
    retry: false,
  });
  return (
    <div className="original-page" data-testid="original-page">
      <OriginalSource pageId={pageId} original={original.data ?? null}
        onOpenMemory={onOpenMemory} onOpenPage={onOpenPage} />
      <OriginalVersions original={original.data ?? null} where="newer" onOpenPage={onOpenPage} />
      {parts.frame ? <OriginalFrame frame={parts.frame} title={title} /> : parts.body && renderMarkdown(parts.body)}
      {parts.notes && <div className="original-notes" data-testid="original-notes">{renderMarkdown(parts.notes)}</div>}
      <OriginalVersions original={original.data ?? null} where="older" onOpenPage={onOpenPage} />
    </div>
  );
}
