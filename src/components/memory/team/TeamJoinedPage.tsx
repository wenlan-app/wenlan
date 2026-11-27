// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { teamJoinedPage, type JoinedTeamPage, type TeamPageMemoryCitation, type TeamPrivateCitation } from "../../../lib/tauri";
import { withoutLeadingTitleH1 } from "../editor/pageEditorPresentation";
import ContentRenderer from "../ContentRenderer";
import { DetailReadingFrame } from "../DetailLayout";
import { WorkspaceBackButton } from "../navigation/WorkspaceNavigation";
import "../readingSurface.css";
import "./joinedLibrary.css";

function relativePublishedTime(iso: string, t: TFunction): string {
  const diff = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
  if (diff < 60) return t("pageDetail.dateline.relativeJustNow");
  if (diff < 3600) return t("pageDetail.dateline.relativeMinutesAgo", { count: Math.floor(diff / 60) });
  if (diff < 86400) return t("pageDetail.dateline.relativeHoursAgo", { count: Math.floor(diff / 3600) });
  return t("pageDetail.dateline.relativeDaysAgo", { count: Math.floor(diff / 86400) });
}

export function TeamJoinedPage({
  teamId,
  pageId,
  onBack,
}: {
  readonly teamId: string;
  readonly pageId: string;
  readonly onBack: () => void;
}) {
  const { t } = useTranslation();
  const query = useQuery({
    queryKey: ["team-joined-page", teamId, pageId],
    queryFn: () => teamJoinedPage(teamId, pageId),
    staleTime: 0,
    retry: false,
  });
  const joinedPage = query.data;
  const unreachable = query.isError || joinedPage?.state === "unreachable";
  const result: Extract<JoinedTeamPage, { state: "ready" }> | null =
    joinedPage?.state === "ready" ? joinedPage : null;

  return (
    <DetailReadingFrame className="page-detail-document team-joined-page">
      <WorkspaceBackButton aria-label={t("main.back")} onClick={onBack}>
        {t("main.back")}
      </WorkspaceBackButton>
      {query.isPending ? <p role="status">{t("team.joinedLoading")}</p> : unreachable ? (
        <div className="team-library-unreachable" role="status">
          <span>{t("team.joinedUnreachable")}</span>
          <button type="button" className="spaces-quiet-action" onClick={() => void query.refetch()}>{t("team.joinedRetry")}</button>
        </div>
      ) : result ? (
        <>
          <div className="page-document-title-row team-joined-page-title-row">
            <h1 className="page-detail-title">{result.page.title}</h1>
          </div>
          <p className="team-joined-published-line">
            {result.page.published_by?.name?.trim() || t("team.joinedPublisherFallback")} {t("team.joinedPublished")}
            {result.page.published_at ? <> · {relativePublishedTime(result.page.published_at, t)}</> : null}
            {" · "}{t("team.joinedReadOnly")}
          </p>
          <div className="page-detail-prose team-joined-page-prose" data-testid="team-joined-page-body">
            <ContentRenderer content={withoutLeadingTitleH1(result.page.body, result.page.title)} variant="detail" preserveMarkdown />
          </div>
          <section className="team-joined-citations" aria-labelledby="team-joined-citations-title">
            <h2 id="team-joined-citations-title" className="note-info-section-heading">{t("team.joinedCitations")}</h2>
            <ul>
              {result.sources.sources.map((source, index) => (
                <li key={`${source.kind}-${index}`}>
                  {source.kind === "private_source"
                    ? t("team.joinedPrivateSource")
                    : (source as TeamPageMemoryCitation).memory.title}
                </li>
              ))}
            </ul>
          </section>
        </>
      ) : null}
    </DetailReadingFrame>
  );
}

export type { TeamPageMemoryCitation, TeamPrivateCitation };
