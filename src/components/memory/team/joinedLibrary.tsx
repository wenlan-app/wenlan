// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { Stack } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { teamJoinedLibrary, type JoinedTeamLibrary, type TeamLibrarySpace, type TeamPageSummary } from "../../../lib/tauri";

export const JOINED_TEAM_LIBRARY_QUERY_KEY = ["team-joined-library"] as const;

export function useJoinedTeamLibrary() {
  return useQuery({
    queryKey: JOINED_TEAM_LIBRARY_QUERY_KEY,
    queryFn: async () => {
      if (typeof teamJoinedLibrary !== "function") return { teams: [] };
      return (await teamJoinedLibrary()) ?? { teams: [] };
    },
    staleTime: 0,
    retry: false,
  });
}

function teamName(team: JoinedTeamLibrary["team"], fallback: string): string {
  return team?.name?.trim() || fallback;
}

export function JoinedTeamLibraryGroups({
  onOpenPage,
}: {
  readonly onOpenPage?: (teamId: string, pageId: string) => void;
}) {
  const { t } = useTranslation();
  const query = useJoinedTeamLibrary();
  const groups = query.data?.teams ?? [];

  if (groups.length === 0) return null;

  return (
    <>
      {groups.map((joined, index) => {
        const name = teamName(joined.team, t("team.unnamed"));
        const teamId = joined.team?.id ?? `joined-team-${index}`;
        return (
          <section className="spaces-team-group team-library-group" data-testid="team-joined-library" key={teamId}>
            <h2 className="spaces-group-title team-library-group-title">{t("team.joinedGroup", { team: name })}</h2>
            {joined.library.state === "unreachable" ? (
              <div className="team-library-unreachable" role="status">
                <span>{t("team.joinedUnreachable")}</span>
                <button type="button" className="spaces-quiet-action" onClick={() => void query.refetch()}>{t("team.joinedRetry")}</button>
              </div>
            ) : (
              <div className="team-library-spaces">
                {(joined.library.team_pages?.length ?? 0) > 0 && (
                  <section className="team-library-space" aria-label={t("team.joinedWholeTeam")}>
                    <h3 className="team-library-space-name">{t("team.joinedWholeTeam")}</h3>
                    <JoinedLibraryPages pages={joined.library.team_pages ?? []} teamId={teamId} onOpenPage={onOpenPage} />
                  </section>
                )}
                {joined.library.spaces.map((space) => (
                  <JoinedLibrarySpace key={space.space.id} space={space} teamId={joined.team?.id} onOpenPage={onOpenPage} />
                ))}
              </div>
            )}
          </section>
        );
      })}
    </>
  );
}

function JoinedLibraryPages({
  pages,
  teamId,
  onOpenPage,
}: {
  readonly pages: readonly TeamPageSummary[];
  readonly teamId?: string;
  readonly onOpenPage?: (teamId: string, pageId: string) => void;
}) {
  return (
    <ul className="team-library-pages">
      {pages.map((page) => (
        <li key={page.id}>
          <button
            type="button"
            className="team-library-page"
            disabled={!teamId || !onOpenPage}
            onClick={() => teamId && onOpenPage?.(teamId, page.id)}
          >
            <Stack aria-hidden="true" size={16} />
            <span>{page.title}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}

function JoinedLibrarySpace({
  space,
  teamId,
  onOpenPage,
}: {
  readonly space: TeamLibrarySpace;
  readonly teamId?: string;
  readonly onOpenPage?: (teamId: string, pageId: string) => void;
}) {
  return (
    <section className="team-library-space" aria-label={space.space.name}>
      <h3 className="team-library-space-name">{space.space.name}</h3>
      {space.pages.length > 0 && (
        <ul className="team-library-pages">
          {space.pages.map((page) => (
            <li key={page.id}>
              <button
                type="button"
                className="team-library-page"
                disabled={!teamId || !onOpenPage}
                onClick={() => teamId && onOpenPage?.(teamId, page.id)}
              >
                <Stack aria-hidden="true" size={16} />
                <span>{page.title}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
