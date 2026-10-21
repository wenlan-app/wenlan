// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPageSessions, type AgentSession } from "../../../lib/tauri";
import { formatSessionDay, sessionPlaceName, sessionToolLabel } from "../../../lib/sessionSource";
import { SessionCard, useSessionDetail } from "./SessionCard";
import "./sessionSources.css";

interface Navigation {
  readonly onOpenMemory?: (sourceId: string) => void;
  readonly onOpenPage?: (pageId: string) => void;
}

interface SessionSourceSectionProps extends Navigation {
  /** Conversations that wrote this item. */
  readonly wrote: readonly AgentSession[];
  /** Conversations that only wrote something a note cites. */
  readonly cited?: readonly AgentSession[];
  /** The detail pane's own heading class, so the section matches its pane. */
  readonly headingClassName?: string;
}

function SessionLine({ session, onOpen }: { session: AgentSession; onOpen: () => void }) {
  const { t, i18n } = useTranslation();
  const place = sessionPlaceName(session);
  const text = [
    t("sessionSource.from", { tool: sessionToolLabel(session.tool) }),
    formatSessionDay(session.first_write_at, i18n.language),
    place,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <button type="button" className="session-source-line" onClick={onOpen}>
      {text}
    </button>
  );
}

/**
 * "Source conversation": where this memory or note came from. One quiet
 * link line per conversation; clicking opens the conversation card. Renders
 * nothing when there is no conversation to show.
 */
export function SessionSourceSection({
  wrote,
  cited = [],
  headingClassName,
  onOpenMemory,
  onOpenPage,
}: SessionSourceSectionProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<AgentSession | null>(null);
  const [citedOpen, setCitedOpen] = useState(false);
  if (wrote.length === 0 && cited.length === 0) return null;

  const headingCount = wrote.length > 0 ? wrote.length : cited.length;
  return (
    <section className="session-source" aria-label={t("sessionSource.title", { count: headingCount })}>
      <h3 className={headingClassName ?? "session-source-heading"}>
        {t("sessionSource.title", { count: headingCount })}
      </h3>
      <ul className="session-source-list">
        {wrote.map((session) => (
          <li key={session.id}>
            <SessionLine session={session} onOpen={() => setOpen(session)} />
          </li>
        ))}
      </ul>
      {cited.length > 0 && (
        <>
          <button
            type="button"
            className="session-source-more"
            aria-expanded={citedOpen}
            onClick={() => setCitedOpen((value) => !value)}
          >
            {t(wrote.length > 0 ? "sessionSource.alsoCites" : "sessionSource.citesOnly", { count: cited.length })}
          </button>
          {citedOpen && (
            <ul className="session-source-list">
              {cited.map((session) => (
                <li key={session.id}>
                  <SessionLine session={session} onOpen={() => setOpen(session)} />
                </li>
              ))}
            </ul>
          )}
        </>
      )}
      {open && (
        <SessionCard
          sessionId={open.id}
          known={open}
          onClose={() => setOpen(null)}
          onOpenMemory={onOpenMemory}
          onOpenPage={onOpenPage}
        />
      )}
    </section>
  );
}

interface MemorySessionSourceProps extends Navigation {
  /** `MemoryItem.session_id`; nothing renders without one. */
  readonly sessionId: string | null | undefined;
  readonly headingClassName?: string;
}

/** The section for a memory: the one conversation that wrote it. */
export function MemorySessionSource({ sessionId, ...rest }: MemorySessionSourceProps) {
  const query = useSessionDetail(sessionId);
  const session = query.data?.session;
  return <SessionSourceSection wrote={session ? [session] : []} {...rest} />;
}

interface PageSessionSourcesProps extends Navigation {
  readonly pageId: string;
  readonly headingClassName?: string;
}

/** The section for a note: conversations that wrote it, then the ones it cites. */
export function PageSessionSources({ pageId, ...rest }: PageSessionSourcesProps) {
  const query = useQuery({
    queryKey: ["page-sessions", pageId],
    queryFn: () => getPageSessions(pageId),
    staleTime: 30_000,
    retry: false,
  });
  const entries = query.data ?? [];
  return (
    <SessionSourceSection
      wrote={entries.filter((entry) => entry.via === "wrote").map((entry) => entry.session)}
      cited={entries.filter((entry) => entry.via === "source").map((entry) => entry.session)}
      {...rest}
    />
  );
}
