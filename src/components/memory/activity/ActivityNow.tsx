// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { Brain, Shapes, Check, Clock, WarningCircle, CircleNotch } from "@phosphor-icons/react";
import { PageIcon } from "../assets/PageIcon";
import { Button } from "../settings/primitives";
import {
  assetSentence,
  blockedCauses,
  knownAssets,
  knownState,
  onDemandPagesWaiting,
  STEP_UNIT,
} from "../../../lib/activitySentence";
import { useActivity } from "../../../lib/useActivity";

import { ACTIVITY_ASSET_ORDER } from "./activityPresentation";

const ROUTES = ["everyday", "synthesis"] as const;
const ASSET_ICONS = { memories: Brain, entities: Shapes, pages: PageIcon };
const displayState = (state: string, onDemand: boolean) =>
  // On demand runs nothing in the background: pending work waits for the
  // user's AI tool, so it is neither running nor blocked.
  onDemand && (state === "running" || state === "blocked")
    ? "waiting"
    : state === "idle" || state === "running" || state === "blocked" ? state : "unknown";

export default function ActivityNow({ onOpenIntelligence }: { readonly onOpenIntelligence?: () => void }) {
  const { t } = useTranslation();
  const query = useActivity();

  if (query.isPending && query.data === undefined) {
    return (
      <section data-testid="activity-now" className="mem-activity-now-surface">
        <h2 className="mem-activity-section-title">{t("activityStatus.nowTitle")}</h2>
        <p>{t("activityStatus.nowLoading")}</p>
      </section>
    );
  }

  if (query.isError) {
    return (
      <section data-testid="activity-now" className="mem-activity-now-surface">
        <h2 className="mem-activity-section-title">{t("activityStatus.nowTitle")}</h2>
        <p role="alert">{t("activityStatus.nowError")}</p>
        <button type="button" onClick={() => void query.refetch()}>
          {t("activityStatus.readAgain")}
        </button>
      </section>
    );
  }

  const activity = query.data;
  if (activity === undefined) return null;

  const byKind = new Map(knownAssets(activity).map(asset => [asset.kind, asset]));
  const assets = ACTIVITY_ASSET_ORDER.flatMap(kind => {
    const asset = byKind.get(kind);
    return asset ? [asset] : [];
  });
  const failed = assets.flatMap((asset) =>
    asset.steps
      .filter((step) => step.failed > 0)
      .map((step) => ({ asset: asset.kind, step: step.name, count: step.failed })),
  );
  const anyFailed = activity.assets.some((asset) =>
    asset.steps.some((step) => step.failed > 0),
  );
  const state = knownState(activity);
  const unknown = state === undefined || activity.assets.some((asset) =>
    asset.kind === "unknown" || asset.state === "unknown" || asset.steps.some((step) =>
      step.name === "unknown" || step.state === "unknown",
    ),
  );

  let summary: "unknown" | "off" | "waiting" | "running" | "blocked" | "idle";
  if (unknown) summary = "unknown";
  else if (state === "off") summary = "off";
  else if (state === "waiting_for_idle") summary = "waiting";
  else if (state === "organizing") summary = "running";
  else if (state === "blocked") summary = "blocked";
  else summary = "idle";

  // blockedCauses names the configured/unavailable route behind blocked asset
  // counts. When there are no asset rows yet, still describe the route state
  // rather than implying the system is idle.
  const causes = blockedCauses(activity);
  if (summary === "blocked" && causes.length === 0) {
    const unavailableJobs = ROUTES.filter((job) => activity[job].mode === "pinned_unavailable");
    const jobs = unavailableJobs.length > 0
      ? unavailableJobs
      : ROUTES.filter((job) => activity[job].mode === "unconfigured");
    for (const job of jobs) {
      const route = activity[job];
      if (route.mode === "unconfigured") {
        causes.push({ key: `activityStatus.blockedCause.${job}` });
      } else if (route.mode === "pinned_unavailable") {
        causes.push({ key: `activityStatus.blockedCauseUnavailable.${job}` });
      }
    }
  }

  const primaryCause = summary === "blocked" ? causes[0] : undefined;
  const additionalCauses = primaryCause ? causes.slice(1) : causes;

  return (
    <section data-testid="activity-now" className="mem-activity-now-surface">
        <h2 className="mem-activity-section-title">{t("activityStatus.nowTitle")}</h2>
      <div className="mem-activity-now-content">
        <p className="mem-activity-now-summary" data-testid={`activity-now-${summary}`}>
          {primaryCause ? t(primaryCause.key, primaryCause.params) : t(({
            unknown: "activityStatus.nowUnknown",
            off: "activityStatus.nowOff",
            waiting: "activityStatus.nowWaiting",
            running: "activityStatus.nowRunning",
            blocked: "activityStatus.nowBlocked",
            idle: "activityStatus.nowIdle",
          } as const)[summary])}
        </p>

        {summary === "off" && onDemandPagesWaiting(activity) > 0 && (
          <p data-testid="activity-now-on-demand">{t("activityStatus.onDemandFinish")}</p>
        )}
        {additionalCauses.map((cause, index) => (
          <p key={`${cause.key}-${index}`}>{t(cause.key, cause.params)}</p>
        ))}
        {summary === "blocked" && onOpenIntelligence && (
          <Button type="button" variant="secondary" size="sm" className="mem-activity-settings-action" onClick={onOpenIntelligence}>
            {t("activityStatus.openSettings")}
          </Button>
        )}
        {failed.length > 0 ? failed.map((item, index) => (
          <p key={`${item.asset}-${item.step}-${index}`} data-testid="activity-now-failed">
            {t(`activityStatus.failedStep.${item.step}`, { count: item.count })}
          </p>
        )) : anyFailed ? (
          <p data-testid="activity-now-failed">{t("activityStatus.failedUnknown")}</p>
        ) : null}
        {assets.length > 0 && (
          <div className="mem-activity-asset-list">
            {assets.map((asset) => {
              const sentence = assetSentence(activity, asset);
              const assetState = displayState(asset.state, summary === "off");
              const AssetIcon = ASSET_ICONS[asset.kind];
              return (
                <section key={asset.kind} className="mem-activity-asset-row" aria-label={t(`activityStatus.asset.${asset.kind}`)}>
                  <div className="mem-activity-asset-heading">
                    <h3><AssetIcon size={17} aria-hidden="true" />{t(`activityStatus.rowAsset.${asset.kind}`)}</h3>
                    <span className={`mem-activity-state mem-activity-state-${assetState}`}>
                      {t(`activityStatus.stepState.${assetState}`)}
                    </span>
                  </div>
                  <div className="mem-activity-step-list">
                    {asset.steps.map((step) => {
                      const count = { key: `activityStatus.compactStepCount.${STEP_UNIT[step.name]}` as const, params: { count: step.total, done: step.done } };
                      const state = displayState(step.state, summary === "off");

                      const measurable = Number.isFinite(step.total) && step.total > 0 && Number.isFinite(step.done) && step.done >= 0;
                      const completed = measurable && step.done >= step.total;
                      const stateLabel = state === "idle" && completed ? t("activityStatus.stepComplete") : t(`activityStatus.stepState.${state}`);
                      const StatusIcon = state === "running" ? CircleNotch : state === "blocked" ? WarningCircle : state === "idle" && completed ? Check : Clock;
                      return (
                        <div key={step.name} className={`mem-activity-step mem-activity-state-${state}`} data-testid={state === "running" ? "activity-now-running-step" : undefined}>
                          <span className="mem-activity-step-label">{t(`activityStatus.step.${step.name}`)}</span>
                          <span className="mem-activity-step-progress">
                            {measurable
                              ? <span className="mem-activity-step-track" aria-hidden="true"><span style={{ width: `${Math.min(100, step.done / step.total * 100)}%` }} /></span>
                              : <span aria-hidden="true">—</span>}
                          </span>
                          <span className="mem-activity-step-state"><StatusIcon size={13} aria-hidden="true" />{stateLabel}</span>
                          <span className="mem-activity-step-count">{t(count.key, count.params)}</span>
                        </div>
                      );
                    })}
                  </div>
                  <p className="mem-activity-asset-context">{t(sentence.key, sentence.params)}</p>
                </section>
              );
            })}
          </div>
        )}
      </div>
    </section>
  );
}
