// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Funnel, CaretRight, Plus } from "@phosphor-icons/react";
import { SourceTypeIcon } from "./SourceTypeIcon";
import type { NoteApp, SourceApp, SourceKind } from "./sourcePresentation";
import type { AssetLens } from "../../../lib/assetLens";
import { AssetLensToggle } from "../assets/AssetLensToggle";
import type { SourceLibraryFilter } from "./SourceLibrary";
import { isSourceApp, SOURCE_APP_NAMES } from "./sourcePresentation";
import "./SourceCollection.css";

export interface SourceCollectionItem {
  id: string;
  title: string;
  metadata: string;
  kind: SourceKind | SourceApp | "folder" | NoteApp;
  onSelect: () => void;
}

export function SourceCollection({ items, lens, label }: { items: SourceCollectionItem[]; lens: AssetLens; label: string }) {
  return <ul className="source-library-list source-collection-list" data-lens={lens} aria-label={label}>
    {items.map((item) => <li key={item.id}>
      <button className="source-library-row" data-source-key={item.id} type="button" onClick={(event) => {
        event.currentTarget.focus({ preventScroll: true });
        item.onSelect();
      }}>
        <span className={`source-library-icon${item.kind === "obsidian" ? " source-library-icon-obsidian" : ""}`}>
          <SourceTypeIcon kind={item.kind} />
        </span>
        <span className="source-library-row-content">
          <span className="source-library-row-title">{item.title}</span>
          <span className="source-library-row-meta">{item.metadata}</span>
        </span>
        {(item.kind === "folder" || item.kind === "obsidian" || item.kind === "logseq") && <CaretRight className="source-collection-chevron" size={16} aria-hidden="true" />}
      </button>
    </li>)}
  </ul>;
}

export function SourceCollectionBreadcrumbs({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  return <nav className="source-collection-breadcrumbs" aria-label={t("sourceContainer.breadcrumb")}>{children}</nav>;
}

export function SourceCollectionToolbar({ search, filter, lens, apps = [], onSearchChange, onFilterChange, onLensChange, onAdd, trailing, breadcrumbs }: {
  search: string;
  /** Apps with saved sources; each gets its own filter, and none shows when nothing was saved from it. */
  apps?: readonly SourceApp[];
  filter: SourceLibraryFilter;
  lens: AssetLens;
  onSearchChange: (value: string) => void;
  onFilterChange: (value: SourceLibraryFilter) => void;
  onLensChange: (value: AssetLens) => void;
  onAdd: () => void;
  trailing?: ReactNode;
  breadcrumbs?: ReactNode;
}) {
  const { t } = useTranslation();
  const filters: SourceLibraryFilter[] = ["all", "files", "links", ...apps, "folders"];
  return <>
    {breadcrumbs}
    <div className="source-library-toolbar collection-toolbar">
      <label className="source-library-search collection-search">
        <Funnel size={18} aria-hidden="true" />
        <input className="collection-search-input" type="search" value={search} onChange={(event) => onSearchChange(event.target.value)} placeholder={t("sourceLibrary.search")} aria-label={t("sourceLibrary.search")} />
      </label>
      <div className="source-library-actions collection-toolbar-actions">
        <AssetLensToggle value={lens} onChange={onLensChange} />
        {trailing}
        <button className="source-library-add collection-control collection-control--primary" onClick={onAdd} type="button"><Plus size={16} aria-hidden="true" />{t("sourceLibrary.add")}</button>
      </div>
    </div>
    <div className="source-library-filters collection-filter-row" role="group" aria-label={t("sourceLibrary.filterLabel")}>
      {filters.map((value) => <button className="collection-control" key={value} aria-pressed={filter === value} onClick={() => onFilterChange(value)}>{isSourceApp(value) ? SOURCE_APP_NAMES[value] : t(`sourceLibrary.filters.${value}`)}</button>)}
    </div>
  </>;
}
