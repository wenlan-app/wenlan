// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { search, type Entity } from "../../../lib/tauri";
import { resolveAgentDisplayName } from "../../../lib/agents";
import { MetadataRow, RailPanelTitle } from "../MemoryDetailPrimitives";
import { EntityTypeControl } from "./EntityTypeControl";
import {
  formatAbsoluteTimestamp,
  formatConfidence,
  formatRelativeEntityTime,
} from "./formatEntityMetadata";

type EntityContextRailProps = {
  readonly entity: Entity;
  readonly locale: string;
  readonly onMemoryClick?: (sourceId: string) => void;
  readonly archived: boolean;
  readonly typePending: boolean;
  readonly onTypeChange: (entityType: string) => Promise<boolean>;
  readonly children?: ReactNode;
};

export function EntityContextRail({ entity, locale, onMemoryClick, archived, typePending, onTypeChange, children }: EntityContextRailProps) {
  const { t } = useTranslation();
  const space = entity.space ?? entity.domain;
  const confidence = formatConfidence(entity.confidence, 2);
  const createdRelative = formatRelativeEntityTime(entity.created_at, locale);
  const createdAbsolute = formatAbsoluteTimestamp(entity.created_at, locale);
  const updatedRelative = formatRelativeEntityTime(entity.updated_at, locale);
  const updatedAbsolute = formatAbsoluteTimestamp(entity.updated_at, locale);
  const { data: linkedMemories = [] } = useQuery({
    queryKey: ["entity-linked-memories", entity.id, entity.name],
    queryFn: async () => {
      const results = await search(entity.name, 10, "memory");
      return results
        .filter((result) => result.entity_id === entity.id || result.score > 0.7)
        .slice(0, 8);
    },
    enabled: Boolean(entity.name),
    staleTime: 30_000,
  });

  return (
    <div
      role="group"
      className="memory-detail-rail page-detail-rail"
      aria-label={t("entityDetail.contextLabel")}
    >
      <section className="memory-detail-rail-section memory-detail-metadata-panel">
        <RailPanelTitle>{t("detailInfo.details")}</RailPanelTitle>
        <div className="memory-detail-metadata-list">
          <MetadataRow label={t("entityDetail.typeLabel")}>
            <EntityTypeControl
              value={entity.entity_type}
              archived={archived}
              pending={typePending}
              onChange={onTypeChange}
            />
          </MetadataRow>
          {space ? (
            <MetadataRow label={t("entityDetail.spaceLabel")}>
              <span>{space}</span>
            </MetadataRow>
          ) : null}
          {confidence ? (
            <MetadataRow label={t("entityDetail.confidenceLabel")}>
              <span>{confidence}</span>
            </MetadataRow>
          ) : null}
          {entity.source_agent ? (
            <MetadataRow label={t("entityDetail.sourceLabel")}>
              <span>{resolveAgentDisplayName(entity.source_agent)}</span>
            </MetadataRow>
          ) : null}
          {createdRelative ? (
            <MetadataRow label={t("entityDetail.firstSeen")}>
              <span title={createdAbsolute ?? undefined}>
                {createdRelative}
              </span>
            </MetadataRow>
          ) : null}
          {updatedRelative ? (
            <MetadataRow label={t("entityDetail.updatedLabel")}>
              <span title={updatedAbsolute ?? undefined}>
                {updatedRelative}
              </span>
            </MetadataRow>
          ) : null}
        </div>
      </section>
      {children}
      {linkedMemories.length > 0 ? (
        <section className="memory-detail-rail-section memory-detail-secondary-panel">
          <div className="memory-detail-panel-heading">
            <RailPanelTitle>{t("detailInfo.relatedMemories")}</RailPanelTitle>
            <span className="entity-appears-hint">{t("detailInfo.relatedMemoriesHint")}</span>
          </div>
          <div className="memory-detail-related-grid">
            {linkedMemories.map((memory) => {
              return (
                <button
                  key={memory.id}
                  type="button"
                  className="memory-detail-related-card"
                  onClick={() => onMemoryClick?.(memory.source_id)}
                >
                  <span className="memory-detail-context-row-body">
                    <span className="memory-detail-related-copy line-clamp-2">
                      {memory.content}
                    </span>
                    {memory.is_archived ? (
                      <span className="memory-detail-related-meta">{t("entityDetail.archived")}</span>
                    ) : null}
                  </span>
                </button>
              );
            })}
          </div>
        </section>
      ) : null}
    </div>
  );
}
