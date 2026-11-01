// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getThing, listThings, moveSessionThing, renameThing, type ThingDetail as ThingRecord, type ThingSession, type ThingSummary } from "../../../lib/tauri";
import { sessionToolLabel } from "../../../lib/sessionSource";
import { relativeTime } from "../../../lib/relativeTime";
import { DetailReadingFrame } from "../DetailLayout";
import { WorkspaceBackButton } from "../navigation/WorkspaceNavigation";
import { THINGS_QUERY_KEY } from "../pages/WorkingOn";
import "./thingDetail.css";

export const thingQueryKey = (id: string) => ["thing", id] as const;

interface ThingDetailProps {
  readonly thingId: string;
  readonly onBack: () => void;
  readonly onOpenPage: (pageId: string) => void;
  readonly onOpenThing: (thingId: string) => void;
}

/**
 * One thing (一件事): where it stands, the conversations sorted into it and
 * the pages they used (docs/ux/spec.md, UX-THING-1, UX-CHAT-5). The name and
 * each conversation's thing change in one step; nothing here asks the person
 * to wrap it up or mark it done.
 */
export function ThingDetail({ thingId, onBack, onOpenPage, onOpenThing }: ThingDetailProps) {
  const { t, i18n } = useTranslation();
  const [moved, setMoved] = useState<ThingSummary | null>(null);
  const { data: thing, isError, refetch } = useQuery({
    queryKey: thingQueryKey(thingId),
    queryFn: () => getThing(thingId),
  });

  if (!thing) {
    return (
      <div className="page-detail thing-detail" aria-label={t("thing.label")}>
        <header className="page-document-title-row">
          <BackButton onBack={onBack} label={t("thing.back")} />
        </header>
        <div className="thing-detail-status" role="status">
          {isError ? (
            <>
              <p>{t("thing.loadError")}</p>
              <button type="button" className="memory-detail-text-button" onClick={() => refetch()}>
                {t("thing.retry")}
              </button>
            </>
          ) : (
            <p>{t("thing.loading")}</p>
          )}
        </div>
      </div>
    );
  }

  const when = relativeTime(thing.last_activity_at, t, i18n.language);
  return (
    <div className="page-detail thing-detail" aria-label={t("thing.label")}>
      <DetailReadingFrame variant="compact" className="page-detail-document">
        <header className="page-document-title-row">
          <ThingTitle key={thing.title ?? ""} thing={thing} />
          <div className="page-document-tools">
            <BackButton onBack={onBack} label={t("thing.back")} />
          </div>
        </header>
        {thing.status_line ? <p className="thing-detail-where">{thing.status_line}</p> : null}
        <p className="thing-detail-meta">{[...thing.spaces, when].join(" · ")}</p>

        <section className="thing-detail-section" aria-labelledby="thing-conversations">
          <h2 id="thing-conversations">{t("thing.conversations")}</h2>
          {moved ? (
            <p className="thing-detail-moved" role="status">
              {t("thing.movedTo", { title: moved.title ?? t("thing.untitled") })}{" "}
              <button type="button" className="memory-detail-text-button" onClick={() => onOpenThing(moved.id)}>
                {t("thing.open")}
              </button>
            </p>
          ) : null}
          {thing.sessions.length === 0 ? (
            <p className="thing-detail-empty">{t("thing.noConversations")}</p>
          ) : (
            <ul className="thing-detail-list">
              {thing.sessions.map((session) => (
                <ConversationRow
                  key={session.id}
                  thing={thing}
                  session={session}
                  onMoved={(to) => {
                    // Moving the last conversation out leaves nothing here, so
                    // go along with it to where it now is.
                    if (thing.sessions.length <= 1) onOpenThing(to.id);
                    else setMoved(to);
                  }}
                />
              ))}
            </ul>
          )}
        </section>

        <section className="thing-detail-section" aria-labelledby="thing-pages">
          <h2 id="thing-pages">{t("thing.pages")}</h2>
          {thing.pages.length === 0 ? (
            <p className="thing-detail-empty">{t("thing.noPages")}</p>
          ) : (
            <ul className="thing-detail-pages">
              {thing.pages.map((page) => (
                <li key={page.id}>
                  <button type="button" className="thing-detail-page" onClick={() => onOpenPage(page.id)}>
                    {page.title}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </DetailReadingFrame>
    </div>
  );
}

/** The thing's name, renamed in place. The AI names new things; a rename here
 *  is the person's and the AI no longer renames it. */
function ThingTitle({ thing }: { readonly thing: ThingRecord }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState(false);
  const shown = thing.title ?? t("thing.untitled");
  const [draft, setDraft] = useState(thing.title ?? "");
  const rename = useMutation({
    mutationFn: (title: string) => renameThing(thing.id, title),
    onSuccess: () => {
      setEditing(false);
      void queryClient.invalidateQueries({ queryKey: thingQueryKey(thing.id) });
      void queryClient.invalidateQueries({ queryKey: THINGS_QUERY_KEY });
    },
  });
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const title = draft.trim();
    if (!title || title === thing.title) {
      setEditing(false);
      return;
    }
    rename.mutate(title);
  };
  if (!editing) {
    return (
      <div className="thing-detail-title-row">
        <h1 className="page-detail-title">{shown}</h1>
        <button type="button" className="memory-detail-text-button thing-detail-rename" onClick={() => { setDraft(thing.title ?? ""); setEditing(true); }}>
          {t("thing.rename")}
        </button>
      </div>
    );
  }
  return (
    <form className="thing-detail-title-form" onSubmit={submit}>
      <input
        className="thing-detail-title-input"
        aria-label={t("thing.nameLabel")}
        value={draft}
        autoFocus
        maxLength={120}
        disabled={rename.isPending}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(false); } }}
      />
      <button type="submit" className="memory-detail-text-button" disabled={rename.isPending}>{t("thing.save")}</button>
      <button type="button" className="memory-detail-text-button" disabled={rename.isPending} onClick={() => setEditing(false)}>{t("thing.cancel")}</button>
      {rename.isError ? <p className="thing-detail-error" role="alert">{t("thing.renameFailed")}</p> : null}
    </form>
  );
}

/** One conversation, with its one-step move to another thing or a new one. */
function ConversationRow({
  thing,
  session,
  onMoved,
}: {
  readonly thing: ThingRecord;
  readonly session: ThingSession;
  readonly onMoved: (to: ThingSummary) => void;
}) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [choosing, setChoosing] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const others = useQuery({ queryKey: THINGS_QUERY_KEY, queryFn: listThings, enabled: choosing });
  const move = useMutation({
    mutationFn: (target: { thingId: string } | { newTitle: string }) => moveSessionThing(session.id, thing.id, target),
    onSuccess: (moved) => {
      setChoosing(false);
      onMoved(moved);
      void queryClient.invalidateQueries({ queryKey: thingQueryKey(thing.id) });
      void queryClient.invalidateQueries({ queryKey: thingQueryKey(moved.id) });
      void queryClient.invalidateQueries({ queryKey: THINGS_QUERY_KEY });
    },
  });
  const title = session.title?.trim() || t("thing.untitledConversation");
  const otherThings = (others.data ?? []).filter((candidate) => candidate.id !== thing.id);
  return (
    <li className="thing-detail-row">
      <div className="thing-detail-row-main">
        <span className="thing-detail-row-title">{title}</span>
        <span className="thing-detail-row-meta">
          {sessionToolLabel(session.tool)} · {relativeTime(session.last_write_at, t, i18n.language)}
        </span>
      </div>
      {choosing ? null : (
        <button type="button" className="memory-detail-text-button" onClick={() => setChoosing(true)}>
          {t("thing.move")}
        </button>
      )}
      {choosing ? (
        <div className="thing-detail-move" role="group" aria-label={t("thing.moveTo", { title })}>
          <p className="thing-detail-move-title">{t("thing.moveTo", { title })}</p>
          {otherThings.length > 0 ? (
            <ul>
              {otherThings.map((candidate) => (
                <li key={candidate.id}>
                  <button
                    type="button"
                    className="thing-detail-move-option"
                    disabled={move.isPending}
                    onClick={() => move.mutate({ thingId: candidate.id })}
                  >
                    {candidate.title ?? t("thing.untitled")}
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          <form
            className="thing-detail-move-new"
            onSubmit={(event) => {
              event.preventDefault();
              const name = newTitle.trim();
              if (name) move.mutate({ newTitle: name });
            }}
          >
            <input
              aria-label={t("thing.newThingLabel")}
              placeholder={t("thing.newThingPlaceholder")}
              value={newTitle}
              maxLength={120}
              disabled={move.isPending}
              onChange={(event) => setNewTitle(event.target.value)}
            />
            <button type="submit" className="memory-detail-text-button" disabled={move.isPending || !newTitle.trim()}>
              {t("thing.newThing")}
            </button>
          </form>
          <button type="button" className="memory-detail-text-button" disabled={move.isPending} onClick={() => setChoosing(false)}>
            {t("thing.cancel")}
          </button>
          {move.isError ? <p className="thing-detail-error" role="alert">{t("thing.moveFailed")}</p> : null}
        </div>
      ) : null}
    </li>
  );
}

function BackButton({ onBack, label }: { readonly onBack: () => void; readonly label: string }) {
  return (
    <WorkspaceBackButton type="button" onClick={onBack} className="memory-detail-back" aria-label={label} title={label}>
      <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <path d="M19 12H5M12 19l-7-7 7-7" />
      </svg>
    </WorkspaceBackButton>
  );
}
