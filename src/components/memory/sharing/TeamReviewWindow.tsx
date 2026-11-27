// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import ContentRenderer from "../ContentRenderer";
import { withoutLeadingTitleH1 } from "../editor/pageEditorPresentation";
import {
  listPages,
  teamLibraryCreateSpace,
  teamLibraryDraft,
  teamLibrarySpaces,
  teamLibraryPreview,
  teamLibraryPublish,
  type Page,
  type TeamGroup,
  type TeamInfo,
  type TeamPublication,
  type TeamPublicationPreview,
  type TeamTerminalReply,
} from "../../../lib/tauri";
import { PAGE_FETCH_LIMIT } from "../space-detail/model";
import { TEAM_LIBRARY_SPACES_KEY, useTeamStatus } from "./team";
import { teamLabel } from "./sharing";
import "./sharing.css";

function errorText(error: unknown, t: TFunction, fallback: string): string {
  const message = error instanceof Error ? error.message : String(error);
  switch (message) {
    case "team_not_allowed": return String(t("team.shareNotAllowed"));
    case "team_page_changed": return String(t("team.shareLoadFailed"));
    case "team_space_taken": return String(t("team.shareSpaceTaken"));
    case "team_unavailable": return String(t("team.shareUnavailable"));
    default: return fallback;
  }
}

function previewReply(reply: TeamTerminalReply, unavailable: string): TeamPublicationPreview {
  if (reply.result !== "preview") throw new Error(unavailable);
  return reply.preview;
}

type TeamReviewWindowProps = {
  readonly pageId: string;
  readonly team: TeamInfo | null | undefined;
  readonly spaces?: readonly TeamGroup[];
  readonly initialSpaceId?: string | null;
  readonly onClose: () => void;
  readonly onPublished?: () => void;
  readonly onSkip?: () => void;
};

export function TeamReviewWindow({ pageId, team, spaces = [], initialSpaceId = null, onClose, onPublished, onSkip }: TeamReviewWindowProps) {
  const { t } = useTranslation();
  const status = useTeamStatus();
  const queryClient = useQueryClient();
  const resolvedTeam = team ?? status.data?.team;
  const librarySpacesQuery = useQuery({
    queryKey: TEAM_LIBRARY_SPACES_KEY,
    queryFn: teamLibrarySpaces,
    retry: false,
    enabled: spaces.length === 0 && Boolean(status.data?.listening),
  });
  const availableSpaces = spaces.length > 0 ? spaces : (librarySpacesQuery.data ?? []);
  const draftQuery = useQuery({
    queryKey: ["team-library-draft", "page", pageId],
    queryFn: () => teamLibraryDraft("page", pageId),
    retry: false,
  });
  const [spaceId, setSpaceId] = useState<string | null>(initialSpaceId);
  const [selectedMemoryIds, setSelectedMemoryIds] = useState<ReadonlySet<string>>(new Set());
  const [previewState, setPreviewState] = useState<{ key: string; value: TeamPublicationPreview } | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const draft = draftQuery.data;
  const selectionKey = useMemo(
    () => `${draft?.item.original.id ?? pageId}@${draft?.item.original.revision ?? "pending"}|${spaceId ?? "team"}|${[...selectedMemoryIds].sort().join(",")}`,
    [draft?.item.original.id, draft?.item.original.revision, pageId, selectedMemoryIds, spaceId],
  );
  const selectedSources = useMemo(
    () => (draft?.sources ?? []).filter((source) => selectedMemoryIds.has(source.original.id)),
    [draft?.sources, selectedMemoryIds],
  );
  const publication = useMemo<TeamPublication | null>(() => {
    if (!draft) return null;
    return {
      kind: "page",
      title: draft.item.text.title,
      body: draft.item.text.body,
      authorship: draft.authorship,
      space_id: spaceId,
      // A new team item: these name an existing team item to revise, never
      // the personal original (that travels in `originals`).
      item_id: null,
      expected_revision: null,
      memories: selectedSources.map((source) => source.text),
      private_source: draft.omitted_sources || selectedSources.length < draft.sources.length,
      brief: false,
    };
  }, [draft, selectedSources, spaceId]);

  useEffect(() => {
    if (!publication) return;
    const currentRequest = ++requestId.current;
    setPreviewState(null);
    setPreviewLoading(true);
    setError(null);
    void teamLibraryPreview(publication, [draft!.item.original, ...selectedSources.map((source) => source.original)])
      .then((reply) => {
        if (currentRequest !== requestId.current) return;
        setPreviewState({ key: selectionKey, value: previewReply(reply, t("team.reviewPreviewUnavailable")) });
      })
      .catch((caught: unknown) => {
        if (currentRequest !== requestId.current) return;
        setError(errorText(caught, t, t("team.shareUnavailable")));
      })
      .finally(() => {
        if (currentRequest === requestId.current) setPreviewLoading(false);
      });
  }, [draft, publication, selectedSources, selectionKey, t]);

  const displayedPreview = previewState?.key === selectionKey ? previewState.value : null;
  const previewReady = displayedPreview !== null && !previewLoading && !publishing;
  const publish = async () => {
    if (!displayedPreview || !previewReady) return;
    setPublishing(true);
    setError(null);
    try {
      const reply = await teamLibraryPublish(displayedPreview.id);
      if (reply.result !== "published") throw new Error(t("team.sharePublishUnavailable"));
      toast.success(t("team.sharePublished", { team: teamLabel(t, resolvedTeam) }));
      void queryClient.invalidateQueries({ queryKey: TEAM_LIBRARY_SPACES_KEY });
      if (onPublished) onPublished();
      else onClose();
    } catch (caught) {
      setError(errorText(caught, t, t("team.shareUnavailable")));
    } finally {
      setPublishing(false);
    }
  };

  return (
    <div className="team-review-window">
      <h2 id="share-dialog-title" className="team-review-title">
        {t("team.reviewTitle", { team: teamLabel(t, resolvedTeam) })}
      </h2>
      {draftQuery.isPending ? <p role="status" className="share-hint">{t("team.reviewLoading")}</p> : null}
      {draftQuery.isError ? <p role="alert" className="share-warning">{errorText(draftQuery.error, t, t("team.shareLoadFailed"))}</p> : null}
      {draft ? (
        <>
          <label className="share-field">
            <span className="share-label">{t("team.reviewDestination")}</span>
            <select className="share-input" value={spaceId ?? ""} onChange={(event) => setSpaceId(event.currentTarget.value || null)}>
              <option value="">{t("team.reviewWholeTeam")}</option>
              {availableSpaces.map((space) => <option key={space.id} value={space.id}>{space.name}</option>)}
            </select>
          </label>
          <div className="share-field">
            <span className="share-label">{t("team.reviewContent")}</span>
            <div className="team-review-content" aria-live="polite">
              {displayedPreview ? (
                <>
                  <h3>{displayedPreview.draft.title}</h3>
                  <div className="team-review-body">
                    <ContentRenderer
                      content={withoutLeadingTitleH1(displayedPreview.draft.body, displayedPreview.draft.title)}
                      variant="detail"
                      allowImages={false}
                    />
                  </div>
                </>
              ) : <span className="share-hint">{t("team.reviewUpdating")}</span>}
            </div>
          </div>
          {draft.sources.length > 0 ? (
            <fieldset className="team-review-sources">
              <legend>{t("team.reviewMemories", { count: draft.sources.length })}</legend>
              <div className="team-review-source-list">
                {draft.sources.map((source) => (
                  <label key={source.original.id} className="team-review-source">
                    <input
                      type="checkbox"
                      checked={selectedMemoryIds.has(source.original.id)}
                      onChange={(event) => {
                        const checked = event.currentTarget.checked;
                        setSelectedMemoryIds((current) => {
                          const next = new Set(current);
                          if (checked) next.add(source.original.id);
                          else next.delete(source.original.id);
                          return next;
                        });
                      }}
                    />
                    <span>{source.text.title}</span>
                  </label>
                ))}
              </div>
              <p className="share-hint">{t("team.reviewPrivateHint")}</p>
            </fieldset>
          ) : null}
          {error ? <p className="share-warning" role="alert">{error}</p> : null}
          <div className="share-actions">
            <button type="button" className="share-secondary" onClick={onClose} disabled={publishing}>{t("sharing.cancel")}</button>
            {onSkip ? <button type="button" className="share-secondary" onClick={onSkip} disabled={publishing}>{t("team.shareSkip")}</button> : null}
            <button type="button" className="share-primary" onClick={() => void publish()} disabled={!previewReady}>
              {t(publishing ? "team.sharePublishing" : "team.sharePublish", { team: teamLabel(t, resolvedTeam) })}
            </button>
          </div>
        </>
      ) : null}
    </div>
  );
}

type TeamSpaceShareFlowProps = {
  readonly spaceName: string;
  readonly onClose: () => void;
};

export function TeamSpaceShareFlow({ spaceName, onClose }: TeamSpaceShareFlowProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const status = useTeamStatus();
  const librarySpacesQuery = useQuery({
    queryKey: TEAM_LIBRARY_SPACES_KEY,
    queryFn: teamLibrarySpaces,
    retry: false,
    enabled: Boolean(status.data?.listening),
  });
  const [teamSpace, setTeamSpace] = useState<TeamGroup | null>(null);
  const [pages, setPages] = useState<Page[]>([]);
  const [selectedPageIds, setSelectedPageIds] = useState<ReadonlySet<string>>(new Set());
  const [currentPageId, setCurrentPageId] = useState<string | null>(null);
  const [finished, setFinished] = useState(false);
  const [creating, setCreating] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const reply = await teamLibraryCreateSpace(spaceName, false);
        if (reply.result !== "space") throw new Error("team_unavailable");
        void queryClient.invalidateQueries({ queryKey: TEAM_LIBRARY_SPACES_KEY });
        const nextPages = await listPages("active", spaceName, PAGE_FETCH_LIMIT);
        if (!active) return;
        setTeamSpace(reply.space);
        setPages(nextPages);
      } catch (caught) {
        if (active) setError(errorText(caught, t, t("team.shareCreateUnavailable")));
      } finally {
        if (active) setCreating(false);
      }
    })();
    return () => { active = false; };
  }, [queryClient, spaceName, t]);

  const selectedPages = pages.filter((page) => selectedPageIds.has(page.id));
  const startReview = () => {
    const first = selectedPages[0];
    if (first) setCurrentPageId(first.id);
    else setCurrentPageId(null);
  };
  const [publishedCount, setPublishedCount] = useState(0);
  const [skippedCount, setSkippedCount] = useState(0);
  const moveOn = (published: boolean) => {
    if (!currentPageId) return;
    if (published) setPublishedCount((count) => count + 1);
    else setSkippedCount((count) => count + 1);
    const remaining = selectedPages.slice(selectedPages.findIndex((page) => page.id === currentPageId) + 1);
    setCurrentPageId(remaining[0]?.id ?? null);
    if (remaining.length === 0) setFinished(true);
  };

  if (currentPageId && teamSpace) {
    return (
      <TeamReviewWindow
        key={currentPageId}
        pageId={currentPageId}
        team={status.data?.team}
        spaces={[teamSpace, ...(librarySpacesQuery.data ?? []).filter((space) => space.id !== teamSpace.id)]}
        initialSpaceId={teamSpace.id}
        onClose={onClose}
        onPublished={() => moveOn(true)}
        onSkip={() => moveOn(false)}
      />
    );
  }

  return (
    <div className="team-review-window">
      <h2 id="share-dialog-title" className="team-review-title">{t("sharing.shareTitle", { name: spaceName })}</h2>
      {creating ? <p role="status" className="share-hint">{t("team.sharePreparing")}</p> : null}
      {error ? <p role="alert" className="share-warning">{error}</p> : null}
      {!creating && !error && !finished && selectedPages.length === 0 && pages.length > 0 ? (
        <p className="share-hint">{t("team.shareSelectPages")}</p>
      ) : null}
      {!creating && !error && !finished && pages.length > 0 && (
        <fieldset className="team-review-sources">
          <legend>{t("team.sharePages")}</legend>
          <div className="team-review-source-list">
            {pages.map((page) => (
              <label key={page.id} className="team-review-source">
                <input
                  type="checkbox"
                  checked={selectedPageIds.has(page.id)}
                  onChange={(event) => {
                    const checked = event.currentTarget.checked;
                    setSelectedPageIds((current) => {
                      const next = new Set(current);
                      if (checked) next.add(page.id);
                      else next.delete(page.id);
                      return next;
                    });
                  }}
                />
                <span>{page.title}</span>
              </label>
            ))}
          </div>
        </fieldset>
      )}
      {!creating && !error && !finished && pages.length === 0 ? <p className="share-hint">{t("team.shareNoPages")}</p> : null}
      {!creating && !error && !finished && selectedPages.length === 0 && pages.length > 0 ? (
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose}>{t("sharing.cancel")}</button>
          <button type="button" className="share-primary" disabled>{t("team.shareNext")}</button>
        </div>
      ) : null}
      {!creating && !error && !finished && selectedPages.length > 0 ? (
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose}>{t("sharing.cancel")}</button>
          <button type="button" className="share-primary" onClick={startReview}>{t("team.shareNext")}</button>
        </div>
      ) : null}
      {!creating && !error && !finished && pages.length === 0 ? (
        <div className="share-actions">
          <button type="button" className="share-primary" onClick={onClose}>{t("sharing.done")}</button>
        </div>
      ) : null}
      {!creating && !error && finished ? (
        <>
          <p role="status" className="share-hint">
            {publishedCount === 0
              ? t("team.shareDoneNonePublished", { skipped: skippedCount })
              : t("team.shareDone", { published: publishedCount, skipped: skippedCount })}
          </p>
          <div className="share-actions"><button type="button" className="share-primary" onClick={onClose}>{t("sharing.done")}</button></div>
        </>
      ) : null}
    </div>
  );
}
