import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  confirmSpace,
  createSpace,
  deleteSpace,
  listSpaces,
  reorderSpace,
  teamHostSpace,
  toggleSpaceStarred,
  updateSpace,
  type Space,
} from "../../../lib/tauri";
import { readAssetLens, writeAssetLens, type AssetLens } from "../../../lib/assetLens";
import { listAllActivePages } from "../pages/listAllPages";
import { Funnel } from "@phosphor-icons/react";
import { AssetLensToggle } from "../assets/AssetLensToggle";
import { JoinSharedDialog } from "../sharing/JoinSharedDialog";
import { useScope } from "../team/scope";
import { PeopleIcon } from "../sharing/SharedWithLine";
import { SHARES_KEY, groupTeamSpaces, teamLabel, useShares } from "../sharing/sharing";
import { TEAM_KEY } from "../sharing/team";
import { ConfirmedSpaces } from "./ConfirmedSpaces";
import { SpaceEditor } from "./SpaceEditor";
import { SuggestedSpaces } from "./SuggestedSpaces";
import { filterSpaces, sortConfirmedSpaces, sortSuggestedSpaces } from "./spaceHelpers";
import type { SpaceRowAction, SpacesOverviewProps } from "./spacesTypes";
import "../assets/assetCards.css";
import "../pages/pageActions.css";
import "./spaces.css";
import "./spacesInventory.css";

const DEPENDENT_QUERY_KEYS = [
  "spaces",
  "spaces-page-counts",
  "pages",
  "sidebar-space-page-counts",
  "space",
  "space-pages",
  "space-memories",
  "space-entities",
  "memories",
] as const;

function assertNever(value: never): never {
  throw new TypeError(`Unsupported spaces action: ${String(value)}`);
}

async function runAction(action: SpaceRowAction): Promise<void> {
  switch (action.kind) {
    case "confirm":
      await confirmSpace(action.space.name);
      return;
    case "create":
      await createSpace(action.value.name, action.value.description);
      return;
    case "delete":
      await deleteSpace(action.space.name);
      return;
    case "rename":
      await updateSpace(action.space.name, action.value.name, action.space.description ?? undefined);
      return;
    case "reorder":
      await reorderSpace(action.space.name, action.target.sort_order);
      return;
    case "star":
      await toggleSpaceStarred(action.space.name);
      return;
    default:
      return assertNever(action);
  }
}

function actionKey(action: SpaceRowAction): string {
  return action.kind === "create" ? "create" : action.space.id;
}

export function SpacesOverview(props: SpacesOverviewProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [joining, setJoining] = useState(false);
  const [filter, setFilter] = useState("");
  const [lens, setLens] = useState<AssetLens>(() => readAssetLens("spaces"));
  const [creating, setCreating] = useState(false);
  const [lastGood, setLastGood] = useState<readonly Space[] | null>(null);
  const [pendingIds, setPendingIds] = useState<readonly string[]>([]);
  const [mutationFailed, setMutationFailed] = useState(false);
  const pendingRef = useRef(new Set<string>());
  const query = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  const shares = useShares();
  const scope = useScope();
  const pagesQuery = useQuery({
    queryKey: ["spaces-page-counts"],
    queryFn: listAllActivePages,
  });
  const pageCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const page of pagesQuery.data ?? []) {
      const spaceName = page.space === undefined ? page.domain?.trim() : page.space?.trim();
      if (spaceName === undefined || spaceName.length === 0) continue;
      const key = spaceName.toLocaleLowerCase();
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }, [pagesQuery.data]);

  useEffect(() => {
    if (query.data !== undefined) setLastGood(query.data);
  }, [query.data]);

  useEffect(() => {
    if (props.createIntent !== true) return;
    setCreating(true);
    props.onCreateIntentHandled?.();
  }, [props.createIntent, props.onCreateIntentHandled]);

  const execute = async (action: SpaceRowAction, key: string): Promise<boolean> => {
    try {
      // The owner of a team whose server is elsewhere: the server makes the
      // Space, and this computer follows it (made here under the server's
      // name, never into a private Space of the same name).
      if (action.kind === "create" && scope.team?.owner && !scope.team.hosted) {
        await teamHostSpace(action.value.name);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
          queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
        ]);
        setCreating(false);
        return true;
      }
      await runAction(action);
      switch (action.kind) {
        case "create":
          // Made on a team's side, by the team's owner: the new Space is the
          // team's at once, as a page started there is.
          if (scope.team?.hosted) {
            await teamHostSpace(action.value.name);
            await Promise.all([
              queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
              queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
            ]);
          }
          setCreating(false);
          break;
        case "delete":
          props.onSpaceDeleted?.(action.space.id);
          break;
        case "rename":
          props.onSpaceRenamed?.({ id: action.space.id, name: action.value.name });
          break;
        case "confirm":
        case "reorder":
        case "star":
          break;
        default:
          assertNever(action);
      }
      await Promise.all(DEPENDENT_QUERY_KEYS.map((key) => queryClient.invalidateQueries({ queryKey: [key] })));
      return true;
    } catch {
      setMutationFailed(true);
      return false;
    } finally {
      pendingRef.current.delete(key);
      setPendingIds(Array.from(pendingRef.current));
    }
  };

  const submit = (action: SpaceRowAction): Promise<boolean> => {
    const key = actionKey(action);
    if (pendingRef.current.has(key)) return Promise.resolve(false);
    pendingRef.current.add(key);
    setPendingIds(Array.from(pendingRef.current));
    setMutationFailed(false);
    return execute(action, key);
  };

  const handleLensChange = (next: AssetLens): void => {
    setLens(next);
    writeAssetLens("spaces", next);
  };

  const allSpaces = query.data ?? lastGood ?? [];
  // Only the side the window shows: Private or one team, never both.
  const suggested = filterSpaces(sortSuggestedSpaces(allSpaces), filter).filter((space) => scope.inScope(space.name));
  const confirmed = filterSpaces(sortConfirmedSpaces(allSpaces), filter).filter((space) => scope.inScope(space.name));
  const { own: ownConfirmed, groups: teamGroups } = scope.teams.length > 0
    ? { own: confirmed, groups: [] }
    : groupTeamSpaces(confirmed, shares.data);
  const hasFilter = filter.trim().length > 0;
  const noResults = hasFilter && suggested.length === 0 && confirmed.length === 0;
  const pendingCreate = pendingIds.includes("create");

  return (
    <div className="spaces-overview">
      <header className="spaces-overview-header collection-toolbar">
        <h1 className="sr-only">{props.labels.title}</h1>
        <label className="spaces-filter collection-search">
          <Funnel size={18} aria-hidden="true" />
          <input
            className="collection-search-input"
            type="search"
            aria-label={props.labels.filterLabel}
            value={filter}
            placeholder={props.labels.filterPlaceholder}
            onChange={(event) => setFilter(event.currentTarget.value)}
          />
        </label>
        <div className="spaces-header-actions collection-toolbar-actions">
          <AssetLensToggle value={lens} onChange={handleLensChange} />
          <SuggestedSpaces
            spaces={suggested}
            labels={props.labels}
            pendingIds={pendingIds}
            onSelect={props.onSelectSpace}
            onKeep={(space) => submit({ kind: "confirm", space })}
            onDiscard={(space) => submit({ kind: "delete", space })}
          />
          {/* Joining a shared folder is Private's. New makes a Space on the
              side shown: a team's comes from its server, so only the team's
              owner makes one there. */}
          {!scope.team && (
            <button type="button" className="spaces-join-action collection-control" onClick={() => setJoining(true)}>
              <PeopleIcon className="spaces-join-icon" />
              {t("sharing.joinEntry")}
            </button>
          )}
          {(!scope.team || scope.team.owner) && (
            <button type="button" className="page-create-action spaces-new-action collection-control collection-control--primary" onClick={() => setCreating(true)}>
              {props.labels.newSpace}
            </button>
          )}
        </div>
      </header>
      {joining ? (
        <JoinSharedDialog
          only="folder"
          onClose={() => setJoining(false)}
          onJoined={(kind, target) => {
            setJoining(false);
            props.onSharedJoined?.(kind, target);
          }}
        />
      ) : null}
      {creating ? (
        <SpaceEditor
          labels={props.labels}
          spaces={allSpaces}
          showDescription
          submitLabel={props.labels.create}
          pending={pendingCreate}
          onCancel={() => setCreating(false)}
          onSubmit={(value) => submit({ kind: "create", value })}
        />
      ) : null}
      {query.isPending && lastGood === null ? <p className="spaces-state">{props.labels.loading}</p> : null}
      {query.isError ? (
        <div className="spaces-error" role="alert">
          <span>{props.labels.loadError}</span>
          <button className="spaces-quiet-action" onClick={() => void query.refetch()}>{props.labels.retry}</button>
        </div>
      ) : null}
      {mutationFailed ? <p className="spaces-error" role="alert">{props.labels.mutationError}</p> : null}
      {query.isPending && lastGood === null ? null : (
        <>
          {ownConfirmed.length > 0 || teamGroups.length === 0 ? (
          <div className={teamGroups.length > 0 ? "spaces-team-group" : undefined}>
            {teamGroups.length > 0 ? <h2 className="spaces-group-title">{t("team.groupMine")}</h2> : null}
          <ConfirmedSpaces
            spaces={ownConfirmed}
            allSpaces={allSpaces}
            labels={props.labels}
            filter={filter}
            noResults={noResults}
            pageCounts={pageCounts}
            pendingIds={pendingIds}
            lens={lens}
            onSelect={props.onSelectSpace}
            onStar={(space) => submit({ kind: "star", space })}
            onRename={(space, value) => submit({ kind: "rename", space, value })}
            onReorder={(space, target) => submit({ kind: "reorder", space, target })}
            onDelete={(space) => submit({ kind: "delete", space })}
          />
          </div>
          ) : null}
          {teamGroups.map((group) => (
            <div key={group.key} className="spaces-team-group">
              <h2 className="spaces-group-title">{teamLabel(t, group.team)}</h2>
          <ConfirmedSpaces
            spaces={group.spaces}
            allSpaces={allSpaces}
            labels={props.labels}
            filter={filter}
            noResults={false}
            pageCounts={pageCounts}
            pendingIds={pendingIds}
            lens={lens}
            onSelect={props.onSelectSpace}
            onStar={(space) => submit({ kind: "star", space })}
            onRename={(space, value) => submit({ kind: "rename", space, value })}
            onReorder={(space, target) => submit({ kind: "reorder", space, target })}
            onDelete={(space) => submit({ kind: "delete", space })}
          />
            </div>
          ))}
          {/* A narrow window has no room for it in the header row. */}
          {!scope.team && (
            <button type="button" className="spaces-join-row spaces-quiet-action" onClick={() => setJoining(true)}>
              <PeopleIcon className="spaces-join-icon" />
              {t("sharing.joinEntry")}
            </button>
          )}
        </>
      )}
    </div>
  );
}
