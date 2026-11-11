// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { Page, SharedStatus } from "../../../lib/tauri";
import type { AssetLens } from "../../../lib/assetLens";
import { AssetLensToggle } from "../assets/AssetLensToggle";
import "../assets/assetCards.css";
import "../assets/collectionToolbar.css";
import type { SpaceDetailCopy } from "./copy";
import { SpaceDossierContent } from "./SpaceDossierContent";

type Props = {
  copy: SpaceDetailCopy;
  spaceId: string;
  pages: readonly Page[];
  pagesPending: boolean;
  pagesError: boolean;
  onRetryPages: () => void;
  onSelectPage: (id: string) => void;
  /** Everything shared, so a row can say who else changed the page. */
  shares?: readonly SharedStatus[];
};

/** A Space's pages under its Brief (UX-SPACE-3). Sources are read inside
 *  each page, so the Space has no tab of its own for them. */
export function SpaceProjectContent({ copy, spaceId, pages, pagesPending, pagesError, onRetryPages, onSelectPage, shares }: Props) {
  const { t } = useTranslation();
  const [lenses, setLenses] = useState<Readonly<Record<string, AssetLens>>>({});
  const lens = lenses[spaceId] ?? "cards";
  const setLens = (next: AssetLens) => setLenses((current) => ({ ...current, [spaceId]: next }));
  return <>
    <div className="space-project-controls collection-toolbar">
      <h2 className="space-project-heading">{copy.metrics.pages}</h2>
      <AssetLensToggle value={lens} onChange={setLens} />
    </div>
    <div>
      {pagesPending ? <p role="status" className="space-dossier-empty">{copy.loading}</p> : pagesError ? <div role="alert"><p className="space-dossier-empty">{copy.relatedLoadError}</p><button type="button" className="page-editor-action" onClick={onRetryPages}>{t("sourceAccess.retry")}</button></div> : <SpaceDossierContent copy={copy} pages={pages} lens={lens} navigation={{ onSelectPage }} shares={shares} />}
    </div>
  </>;
}
