import type { Space } from "../../../lib/tauri";
import { TabEmptyMark } from "../../onboarding/TabIntro";
import type { AssetLens } from "../../../lib/assetLens";
import { findReorderTarget } from "./spaceHelpers";
import { SpaceCard } from "./SpaceCard";
import { SpaceRow } from "./SpaceRow";
import { useSpaceReorderDrag } from "./useSpaceReorderDrag";
import type { SpacesOverviewLabels, SpaceEditorValue } from "./spacesTypes";

type ConfirmedSpacesProps = {
  readonly spaces: readonly Space[];
  readonly allSpaces: readonly Space[];
  readonly labels: SpacesOverviewLabels;
  readonly filter: string;
  readonly noResults: boolean;
  readonly pageCounts: ReadonlyMap<string, number>;
  readonly pendingIds: readonly string[];
  readonly lens: AssetLens;
  readonly onSelect: (name: string) => void;
  readonly onStar: (space: Space) => void;
  readonly onRename: (space: Space, value: SpaceEditorValue) => Promise<boolean>;
  readonly onReorder: (space: Space, target: Space) => void | boolean | Promise<boolean>;
  readonly onDelete: (space: Space) => void;
};

export function ConfirmedSpaces(props: ConfirmedSpacesProps) {
  const identity = JSON.stringify([
    props.filter,
    props.lens,
    props.spaces.map(({ id, name, description, sort_order, starred }) => [id, name, description, sort_order, starred]),
  ]);
  const drag = useSpaceReorderDrag(props.spaces, props.pendingIds.length > 0, identity, props.onReorder);

  const requestMove = (space: Space, direction: "up" | "down") => {
    const target = findReorderTarget(props.spaces, space.id, direction);
    if (target !== null) props.onReorder(space, target);
  };

  return (
    <section className="spaces-section spaces-confirmed-section" aria-label={props.labels.confirmedHeading}>
      {props.noResults ? (
        <p className="spaces-empty">{props.labels.noResults}</p>
      ) : props.spaces.length === 0 ? (
        props.filter.trim() ? null : <p className="spaces-empty"><TabEmptyMark tab="spaces" />{props.labels.noConfirmed}</p>
      ) : props.lens === "cards" ? (
        <div className="asset-cards" data-testid="spaces-cards">
          {props.spaces.map((space) => (
            <SpaceCard
              key={space.id}
              space={space}
              spaces={props.allSpaces}
              labels={props.labels}
              pageCount={props.pageCounts.get(space.name.toLocaleLowerCase()) ?? 0}
              pending={props.pendingIds.includes(space.id)}
              canMoveUp={props.pendingIds.length === 0 && findReorderTarget(props.spaces, space.id, "up") !== null}
              canMoveDown={props.pendingIds.length === 0 && findReorderTarget(props.spaces, space.id, "down") !== null}
              onSelect={props.onSelect}
              onStar={props.onStar}
              onRename={props.onRename}
              onMoveUp={(source) => requestMove(source, "up")}
              onMoveDown={(source) => requestMove(source, "down")}
              onDelete={props.onDelete}
            />
          ))}
        </div>
      ) : (
        <div className="spaces-rows">
          {props.spaces.map((space) => {
            const canMoveUp = props.pendingIds.length === 0 && findReorderTarget(props.spaces, space.id, "up") !== null;
            const canMoveDown = props.pendingIds.length === 0 && findReorderTarget(props.spaces, space.id, "down") !== null;
            return (
              <div key={space.id} {...drag.rowProps(space)} onClickCapture={drag.onClickCapture}>
                <SpaceRow
                  space={space}
                  spaces={props.allSpaces}
                  labels={props.labels}
                  pageCount={props.pageCounts.get(space.name.toLocaleLowerCase()) ?? 0}
                  pending={props.pendingIds.includes(space.id)}
                  canMoveUp={canMoveUp}
                  canMoveDown={canMoveDown}
                  onSelect={props.onSelect}
                  onStar={props.onStar}
                  onRename={props.onRename}
                  onMoveUp={(source) => requestMove(source, "up")}
                  onMoveDown={(source) => requestMove(source, "down")}
                  onDelete={props.onDelete}
                  onDragStart={(event) => drag.begin(event, space)}
                  dragDisabled={props.pendingIds.length > 0 || (!canMoveUp && !canMoveDown)}
                  dragActive={drag.dragActive}
                />
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}
