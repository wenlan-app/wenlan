// SPDX-License-Identifier: AGPL-3.0-only
import type { CSSProperties, Ref } from "react";
import { ArrowRight } from "@phosphor-icons/react";
import "./activitySummary.css";
import { useTranslation } from "react-i18next";
import type { ActivityResponse } from "../../../lib/tauri";
import { relativeTime } from "../../../lib/relativeTime";
import { assetCount, assetProgress, assetSentence, knownAssets, onDemandPagesWaiting, type KnownActivityAsset, type KnownActivityAssetKind } from "../../../lib/activitySentence";

import { ACTIVITY_ASSET_ORDER } from "./activityPresentation";
const ASSET_COLOR: Record<KnownActivityAssetKind, string> = {
  memories: "var(--mem-accent-indigo)",
  entities: "var(--mem-accent-sage)",
  pages: "var(--mem-accent-warm)",
};

function AssetRow({ activity, asset }: { readonly activity: ActivityResponse; readonly asset: KnownActivityAsset }) {
  const { t } = useTranslation();
  const phrase = assetSentence(activity, asset);
  const fraction = assetProgress(asset);
  const summaryKey = phrase.key === "activityStatus.assetDone.pages"
    ? "activityStatus.summary.pagesCurrent"
    : phrase.key === "activityStatus.assetBlockedNoModel.memories"
      ? "activityStatus.summary.memoriesWaiting"
      : phrase.key === "activityStatus.assetBlockedNoModel.entities"
        ? "activityStatus.summary.entitiesWaiting"
        : phrase.key;

  return (
    <div className="mem-activity-summary-row" data-testid={`activity-asset-${asset.kind}`}
      style={{ "--activity-asset-color": ASSET_COLOR[asset.kind] } as CSSProperties}>
      <div className="mem-activity-summary-heading">
        <span className="mem-activity-summary-dot" aria-hidden="true" />
        <span className="mem-activity-summary-name">{t(`activityStatus.asset.${asset.kind}`)}</span>
        <span className="mem-activity-summary-count" data-testid={`activity-asset-count-${asset.kind}`}>{assetCount(asset)}</span>
      </div>
      <p className="mem-activity-summary-description">{t(summaryKey, phrase.params)}</p>
      {activity.state !== "off" && (
        <div className="mem-activity-summary-track" aria-hidden="true">
          <div data-testid={`activity-asset-bar-${asset.kind}`} data-fraction={fraction}
            style={{ width: `${Math.round(fraction * 100)}%` }} />
        </div>
      )}
    </div>
  );
}

interface Props {
  readonly id: string;
  readonly activity?: ActivityResponse;
  readonly isError: boolean;
  readonly firstActionRef: Ref<HTMLButtonElement>;
  readonly onReadAgain: () => void;
  readonly onOpenActivity: () => void;
}

export default function ActivitySummaryPopover({ id, activity, isError, firstActionRef, onReadAgain, onOpenActivity }: Props) {
  const { t, i18n } = useTranslation();
  const byKind = new Map(activity ? knownAssets(activity).map((asset) => [asset.kind, asset]) : []);
  return (
    <div id={id} className="mem-popover-surface mem-activity-popover" data-testid="activity-summary" role="dialog" aria-label={t("activityStatus.statusLabel")}>
      <h2 className="mem-activity-summary-title">{t("main.activity")}</h2>
      {isError ? (
        <div>
          <p role="alert">{t("activityStatus.nowError")}</p>
          <button ref={firstActionRef} type="button" className="mem-activity-popover-action" onClick={onReadAgain}>{t("activityStatus.readAgain")}</button>
        </div>
      ) : activity === undefined ? (
        <p role="status">{t("activityStatus.nowLoading")}</p>
      ) : (
        <>
          {activity.state === "off" && (
            <p className="mem-activity-summary-mode" data-testid="activity-summary-on-demand">
              <span className="mem-activity-summary-mode-name">{t("activityStatus.state.off")}</span>
              {onDemandPagesWaiting(activity) > 0 && <span>{t("activityStatus.onDemandFinish")}</span>}
            </p>
          )}
          <div className="mem-activity-summary-list">
            {ACTIVITY_ASSET_ORDER.map((kind) => {
              const asset = byKind.get(kind);
              return asset ? <AssetRow key={kind} activity={activity} asset={asset} /> : null;
            })}
          </div>
        </>
      )}
      <footer className="mem-activity-summary-footer">
        {!isError && activity !== undefined && (
          <span className="mem-activity-summary-last" data-testid="activity-summary-last">
            {activity.last_activity_at === null ? t("activityStatus.neverActive") : t("activityStatus.lastActivity", { time: relativeTime(activity.last_activity_at, t, i18n.language) })}
          </span>
        )}
        <button ref={isError ? undefined : firstActionRef} type="button" className="mem-activity-popover-action mem-activity-summary-link" data-testid="activity-summary-open" onClick={onOpenActivity}>
          {t("activityStatus.openActivity")}<ArrowRight size={14} aria-hidden="true" />
        </button>
      </footer>
    </div>
  );
}
