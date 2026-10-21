// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { MagnifyingGlass } from "@phosphor-icons/react";
import {
  getSessionDetail,
  type AgentSession,
  type SessionDetail,
  type SessionReadEntry,
} from "../../../lib/tauri";
import {
  formatSessionClock,
  formatSessionDay,
  sessionRepoName,
  sessionToolLabel,
  shortCommit,
} from "../../../lib/sessionSource";
import { ReferenceTypeIcon } from "../links/ReferenceTypeIcon";
import { SessionResumeAction } from "./SessionResumeAction";
import "./sessionSources.css";

/** Shared by the section line and the card so one fetch serves both. */
export const sessionDetailKey = (sessionId: string) => ["session-detail", sessionId] as const;

export function useSessionDetail(sessionId: string | null | undefined) {
  return useQuery({
    queryKey: sessionDetailKey(sessionId ?? ""),
    queryFn: () => getSessionDetail(sessionId as string),
    enabled: Boolean(sessionId),
    staleTime: 30_000,
    retry: false,
  });
}

const LIST_SHOWN = 8;
/** Lookups listed before "N more". */
const READS_SHOWN = 3;

/** The conversation's own title, when it has one. */
function sessionTitle(session: AgentSession | undefined): string {
  return session?.title?.trim() ?? "";
}

interface SessionCardProps {
  readonly sessionId: string;
  /** What the opener already knows, so the title is right while loading. */
  readonly known?: AgentSession;
  readonly onClose: () => void;
  readonly onOpenMemory?: (sourceId: string) => void;
  readonly onOpenPage?: (pageId: string) => void;
}

function useTimeSpan(session: AgentSession): string {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;
  const start = session.first_write_at;
  const end = session.last_write_at;
  const day = formatSessionDay(start, locale);
  const startClock = formatSessionClock(start, locale);
  if (!day) return "";
  if (end - start < 60) return t("sessionSource.timeOnce", { day, start: startClock });
  const endClock = formatSessionClock(end, locale);
  if (new Date(start * 1000).toDateString() === new Date(end * 1000).toDateString()) {
    return t("sessionSource.timeSpan", { day, start: startClock, end: endClock });
  }
  return t("sessionSource.timeSpanDays", {
    startDay: day,
    start: startClock,
    endDay: formatSessionDay(end, locale),
    end: endClock,
  });
}

function PlaceLine({ session }: { session: AgentSession }) {
  const { t } = useTranslation();
  const repo = sessionRepoName(session);
  const parts = [
    repo ? t("sessionSource.repo", { name: repo }) : null,
    session.repo_branch ? t("sessionSource.branch", { name: session.repo_branch }) : null,
    shortCommit(session.repo_commit)
      ? t("sessionSource.commit", { name: shortCommit(session.repo_commit) })
      : null,
  ].filter((part): part is string => Boolean(part));
  if (parts.length === 0) return null;
  // Each "label value" pair stays on one line with the dot before it, so a
  // wrap never leaves a dot hanging at the end of a line.
  return (
    <p className="session-card-muted">
      {parts.map((part, index) => (
        <span key={part}>
          {index > 0 && " "}
          <span className="session-card-nowrap">{index > 0 ? `· ${part}` : part}</span>
        </span>
      ))}
    </p>
  );
}

function CappedList({
  label,
  kind,
  items,
}: {
  label: string;
  kind: "memory" | "page";
  items: ReadonlyArray<{ key: string; title: string; onOpen: () => void }>;
}) {
  const { t } = useTranslation();
  const [all, setAll] = useState(false);
  const shown = all ? items : items.slice(0, LIST_SHOWN);
  return (
    <div className="session-card-group">
      <h4 className="session-card-subheading">{label}</h4>
      <ul className="session-card-list" aria-label={label}>
        {shown.map((item) => (
          <li key={item.key}>
            <button type="button" className="session-card-row" onClick={item.onOpen}>
              <span className="session-card-row-icon"><ReferenceTypeIcon kind={kind} size={14} /></span>
              <span className="session-card-row-title">{item.title}</span>
            </button>
          </li>
        ))}
      </ul>
      {items.length > LIST_SHOWN && (
        <button type="button" className="session-card-more" aria-expanded={all} onClick={() => setAll((v) => !v)}>
          {all ? t("sessionSource.showFewer") : t("sessionSource.showAll", { count: items.length })}
        </button>
      )}
    </div>
  );
}

function ReadsSection({
  reads,
  total,
  onOpenPage,
}: {
  reads: ReadonlyArray<SessionReadEntry>;
  total: number;
  onOpenPage?: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const untitled = t("knowledgeContext.untitled");
  const shown = reads.slice(0, READS_SHOWN);
  const more = Math.max(total, reads.length) - shown.length;
  const heading = t("sessionSource.readsTitle");
  return (
    <section className="session-card-section">
      <h3 className="session-card-heading">{heading}</h3>
      <ul className="session-card-list" aria-label={heading}>
        {shown.map((read, index) => {
          const key = `${read.kind}:${read.label}:${read.created_at}:${index}`;
          if (read.kind === "read_page") {
            const label = t("sessionSource.readPage", { title: read.title?.trim() || untitled });
            return (
              <li key={key}>
                <button type="button" className="session-card-row" onClick={() => onOpenPage?.(read.label)}>
                  <span className="session-card-row-icon"><ReferenceTypeIcon kind="page" size={14} /></span>
                  <span className="session-card-row-title">{label}</span>
                </button>
              </li>
            );
          }
          const label = read.kind === "brief"
            ? t("sessionSource.readBrief", { space: read.label })
            : t("sessionSource.readRecall", { query: read.label });
          return (
            <li key={key} className="session-card-row session-card-row-static">
              <span className="session-card-row-icon"><MagnifyingGlass size={14} aria-hidden="true" focusable="false" /></span>
              <span className="session-card-row-title">{label}</span>
            </li>
          );
        })}
      </ul>
      {more > 0 && <p className="session-card-muted">{t("sessionSource.readsMore", { count: more })}</p>}
    </section>
  );
}

function CardBody({
  detail,
  onOpenMemory,
  onOpenPage,
}: {
  detail: SessionDetail;
  onOpenMemory?: (sourceId: string) => void;
  onOpenPage?: (pageId: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const { session } = detail;
  const span = useTimeSpan(session);
  // The daemon lists every version a session wrote; a note is one thing.
  const seenPages = new Set<string>();
  const pages = detail.pages.filter((page) => {
    if (seenPages.has(page.id)) return false;
    seenPages.add(page.id);
    return true;
  });
  const untitled = t("knowledgeContext.untitled");
  const memoryItems = detail.memories.map((memory) => ({
    key: memory.source_id,
    title: memory.title || untitled,
    onOpen: () => onOpenMemory?.(memory.source_id),
  }));
  const pageItems = pages.map((page) => ({
    key: page.id,
    title: page.title || untitled,
    onOpen: () => onOpenPage?.(page.id),
  }));
  const hasLeft = memoryItems.length > 0 || pageItems.length > 0;
  const reads = detail.reads ?? [];
  const citedItems = (detail.cited_by ?? []).map((note) => ({
    key: note.id,
    title: note.title || untitled,
    onOpen: () => onOpenPage?.(note.id),
  }));
  // With a conversation title as the heading, the tool moves to the meta line.
  const tool = sessionTitle(session) ? sessionToolLabel(session.tool) : "";
  const meta = [tool, span].filter(Boolean).join(" · ");

  return (
    <>
      {meta && <p className="session-card-muted">{meta}</p>}
      <PlaceLine session={session} />
      {detail.handoffs.length > 0 && (
        <section className="session-card-section">
          <h3 className="session-card-heading">{t("sessionSource.handoffTitle")}</h3>
          <ul className="session-card-handoffs">
            {detail.handoffs.map((handoff) => (
              <li key={`${handoff.space}:${handoff.brief_version}`}>
                <p className="session-card-handoff-text">{handoff.summary}</p>
                <p className="session-card-muted">
                  {[handoff.space, formatSessionDay(handoff.created_at, i18n.language)]
                    .filter(Boolean)
                    .join(" · ")}
                </p>
              </li>
            ))}
          </ul>
        </section>
      )}
      {reads.length > 0 && (
        <ReadsSection reads={reads} total={detail.reads_total ?? reads.length} onOpenPage={onOpenPage} />
      )}
      {hasLeft && (
        <section className="session-card-section">
          <h3 className="session-card-heading">{t("sessionSource.leftTitle")}</h3>
          {memoryItems.length > 0 && (
            <CappedList
              label={t("sessionSource.memories", { count: memoryItems.length })}
              kind="memory"
              items={memoryItems}
            />
          )}
          {pageItems.length > 0 && (
            <CappedList
              label={t("sessionSource.notes", { count: pageItems.length })}
              kind="page"
              items={pageItems}
            />
          )}
        </section>
      )}
      {citedItems.length > 0 && (
        <section className="session-card-section">
          <h3 className="session-card-heading">{t("sessionSource.citedByTitle")}</h3>
          <CappedList label={t("sessionSource.notes", { count: citedItems.length })} kind="page" items={citedItems} />
        </section>
      )}
      <SessionResumeAction session={session} transcriptAvailable={detail.transcript_available === true} />
    </>
  );
}

/**
 * The drill-in for one AI conversation: when and where it ran, the Brief
 * summaries it handed over, and what it left in the library. Never shows the
 * transcript's location.
 */
export function SessionCard({ sessionId, known, onClose, onOpenMemory, onOpenPage }: SessionCardProps) {
  const { t } = useTranslation();
  const query = useSessionDetail(sessionId);
  const panel = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const detail = query.data ?? null;
  const shownSession = detail?.session ?? known;
  const tool = sessionToolLabel(shownSession?.tool ?? "");
  const conversationTitle = sessionTitle(shownSession);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    panel.current?.focus();
    return () => { if (previous?.isConnected) previous.focus(); };
  }, []);

  const leave = (open?: (id: string) => void) => (id: string) => {
    onClose();
    open?.(id);
  };

  return (
    <div className="session-card-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div
        ref={panel}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="session-card-panel"
        onKeyDown={(event) => {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(); return; }
          if (event.key !== "Tab") return;
          const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? []);
          const first = controls[0];
          const last = controls[controls.length - 1];
          if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) {
            event.preventDefault(); last?.focus();
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault(); first?.focus();
          }
        }}
      >
        <div className="session-card-body">
          <h2 id={titleId} className="session-card-title">
            {conversationTitle ||
              (tool ? t("sessionSource.cardTitle", { tool }) : t("sessionSource.title", { count: 1 }))}
          </h2>
          {query.isPending && <p role="status" className="session-card-muted">{t("sessionSource.loading")}</p>}
          {!query.isPending && !detail && <p role="alert" className="session-card-muted">{t("sessionSource.loadFailed")}</p>}
          {detail && (
            <CardBody detail={detail} onOpenMemory={leave(onOpenMemory)} onOpenPage={leave(onOpenPage)} />
          )}
        </div>
        <div className="session-card-actions">
          <button type="button" className="session-card-close" onClick={onClose}>{t("common.close")}</button>
        </div>
      </div>
    </div>
  );
}
