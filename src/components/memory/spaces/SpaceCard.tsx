import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { Space } from "../../../lib/tauri";
import { AssetCard } from "../assets/AssetCard";
import { SpaceMark } from "../navigation/SpaceMark";
import { SpaceActionsMenu } from "./SpaceActionsMenu";
import { SpaceEditor } from "./SpaceEditor";
import type { SpacesOverviewLabels, SpaceEditorValue } from "./spacesTypes";

type SpaceCardProps = {
  readonly space: Space;
  readonly spaces: readonly Space[];
  readonly labels: SpacesOverviewLabels;
  readonly pageCount: number;
  readonly pending: boolean;
  readonly onSelect: (name: string) => void;
  readonly onStar: (space: Space) => void;
  readonly onRename: (space: Space, value: SpaceEditorValue) => Promise<boolean>;
  /** Asks whether to delete; the list owns the question. */
  readonly onDelete: (space: Space) => void;
};

export function SpaceCard(props: SpaceCardProps) {
  const { t } = useTranslation();
  const [renaming, setRenaming] = useState(false);

  if (renaming) {
    return (
      <div className="spaces-row spaces-row-edit space-card-edit" data-testid={`space-card-${props.space.id}`}>
        <SpaceEditor
          labels={props.labels}
          spaces={props.spaces}
          initialName={props.space.name}
          initialDescription={props.space.description ?? ""}
          showDescription={false}
          submitLabel={props.labels.save}
          pending={props.pending}
          onCancel={() => setRenaming(false)}
          onSubmit={async (value) => {
            if (await props.onRename(props.space, value)) setRenaming(false);
          }}
        />
      </div>
    );
  }

  return (
    <AssetCard
      testId={`space-card-${props.space.id}`}
      title={props.space.name}
      context={props.space.description}
      footer={(
        <>
          <span className="space-card-counts">
            <span data-testid="space-card-pages">{t("spaces.overview.card.pages", { count: props.pageCount })}</span>
          </span>
        </>
      )}
      onOpen={() => props.onSelect(props.space.name)}
      openArea="card"
      openLabel={t("spaces.overview.card.open", { name: props.space.name })}
    >
      <div className="space-card-head">
        <span className="space-card-mark" aria-hidden="true">
          <SpaceMark active={props.space.starred} />
        </span>
        {props.space.starred ? <span className="spaces-star" aria-hidden="true">★</span> : null}
        <div className="spaces-menu-anchor">
          <SpaceActionsMenu
            space={props.space}
            labels={props.labels}
            pending={props.pending}
            onStar={props.onStar}
            onRename={() => setRenaming(true)}
            onDelete={props.onDelete}
          />
        </div>
      </div>
    </AssetCard>
  );
}
