// SPDX-License-Identifier: AGPL-3.0-only
//
// The pure state-to-copy mapping behind the toolbar Activity button, the popover and the
// Now section. Kept out of the components so acceptance 5 (Blocked is honest)
// and 6 (trust matches routing) are tested as functions with literal inputs
// rather than through three render trees that would each have to be mocked.
//
// Every function here returns an i18n key plus its interpolation params. None
// of them return prose: the copy lives in activityCopy.ts, and
// hardcodedCopyGuard.test.ts would fail on a literal sentence here.

import type { ParseKeys } from "i18next";
import type {
  ActivityAssetKind,
  ActivityAssetStatus,
  ActivityLane,
  ActivityResponse,
  ActivityRoute,
  ActivityState,
  ActivityStep,
  ActivityStepName,
} from "./tauri";

/**
 * An i18n key with its interpolation params. `key` is `ParseKeys`, not
 * `string`, so a typo or a renamed copy key fails `tsc` here rather than
 * rendering the key itself into the sidebar.
 */
export interface Phrase {
  readonly key: ParseKeys;
  readonly params?: Record<string, string | number>;
}

// ── Words this build knows ─────────────────────────────────────────────────
//
// A daemon newer than this app can send a word in any of the response's enums
// that this build has no copy for. The Rust client reads it as "unknown"; the
// browser preview proxies the daemon's JSON untouched, so it can arrive as the
// raw new word. Either way it is not in this build's tables, and every surface
// reads the response through the narrowing below so no lookup ever sees it.

export type KnownActivityState = Exclude<ActivityState, "unknown">;
export type KnownActivityAssetKind = Exclude<ActivityAssetKind, "unknown">;
export type KnownActivityStepName = Exclude<ActivityStepName, "unknown">;
export type KnownActivityLane = Exclude<ActivityLane, "unknown">;
/** The two jobs, by the response field that carries each route. */
export type ActivityRouteJob = "everyday" | "synthesis";
export const ROUTE_JOBS: readonly ActivityRouteJob[] = ["everyday", "synthesis"];

export type KnownActivityStep = Omit<ActivityStep, "name"> & {
  readonly name: KnownActivityStepName;
};
export type KnownActivityAsset = Omit<ActivityAssetStatus, "kind" | "steps"> & {
  readonly kind: KnownActivityAssetKind;
  readonly steps: KnownActivityStep[];
};

function has<K extends string>(table: Readonly<Record<K, unknown>>, word: string): word is K {
  return Object.prototype.hasOwnProperty.call(table, word);
}

const STATE_WORDS: Readonly<Record<KnownActivityState, true>> = {
  up_to_date: true,
  off: true,
  organizing: true,
  waiting_for_idle: true,
  blocked: true,
};

/**
 * The state, or undefined when a newer daemon sent one this app predates.
 * An unknown state makes no claim: the toolbar shows a plain clock and the
 * headlines drop, while the rows, models and trust sentence still read from
 * the rest of the response.
 */
export function knownState(activity: ActivityResponse): KnownActivityState | undefined {
  return has(STATE_WORDS, activity.state) ? activity.state : undefined;
}

/**
 * The assets and steps this build can name. An asset group or a step it has
 * no words for is left out rather than shown as a raw key: its counts are in
 * a unit this build cannot name, so no row here could say what they count.
 */
export function knownAssets(activity: ActivityResponse): KnownActivityAsset[] {
  const assets: KnownActivityAsset[] = [];
  for (const asset of activity.assets) {
    const kind = asset.kind;
    if (!has(OWN_STEP, kind)) continue;
    const steps: KnownActivityStep[] = [];
    for (const step of asset.steps) {
      const name = step.name;
      if (has(STEP_UNIT, name)) steps.push({ ...step, name });
    }
    assets.push({ ...asset, kind, steps });
  }
  return assets;
}

/**
 * The job whose lane governs an asset. Memories and Entities run on the
 * everyday route; Pages on synthesis. This mirrors `ActivityStepName::job()`
 * in wenlan-types, which is the server-side authority — Store and Confirm
 * carry no lane, so an asset is governed by whichever of its steps does.
 */
export function governingJob(kind: KnownActivityAssetKind): ActivityRouteJob {
  return kind === "pages" ? "synthesis" : "everyday";
}

export function routeFor(
  activity: ActivityResponse,
  kind: KnownActivityAssetKind,
): ActivityRoute {
  return governingJob(kind) === "synthesis"
    ? activity.synthesis
    : activity.everyday;
}

/**
 * The step that counts an asset's OWN items: memories for Memories, entities
 * for Entities, pages for Pages.
 *
 * The asset's `done`/`total` follow whichever step is busy, and the steps do
 * not share a unit: Detect counts memories scanned, Confirm counts entities.
 * One memory can name several entities or none, so a memory count printed
 * beside "Entities" is a wrong number, not a rounding.
 */
const OWN_STEP: Readonly<Record<KnownActivityAssetKind, KnownActivityStepName>> = {
  memories: "store",
  entities: "confirm",
  pages: "write",
};

function findStep(
  asset: KnownActivityAsset,
  name: KnownActivityStepName,
): KnownActivityStep | undefined {
  return asset.steps.find((step) => step.name === name);
}

/**
 * The unit a step counts in: an asset's own items, or passages for Embed,
 * which prepares memory rows (one memory can be several passages).
 */
export type StepUnit = KnownActivityAssetKind | "passages";

/** What one step counts. Detect scans memories; Confirm settles entities. */
export const STEP_UNIT: Readonly<Record<KnownActivityStepName, StepUnit>> = {
  store: "memories",
  embed: "passages",
  summarize: "memories",
  link: "memories",
  detect: "memories",
  confirm: "entities",
  write: "pages",
};

/**
 * Steps that run in every mode, On demand and Off included. Embed prepares
 * passages for search by meaning with the built-in embedder, not the user's
 * AI model, so it is never "waiting for your AI tool" and never blocked.
 */
const RUNS_IN_EVERY_MODE: ReadonlySet<KnownActivityStepName> = new Set(["embed"]);

export function runsInEveryMode(name: KnownActivityStepName): boolean {
  return RUNS_IN_EVERY_MODE.has(name);
}

/**
 * Passages a big import stored that search by meaning cannot find yet:
 * what the running Embed step still has to do. Zero when nothing waits, the
 * step is idle, or a daemon predates the step.
 */
export function passagesBeingPrepared(activity: ActivityResponse | undefined): number {
  if (activity === undefined) return 0;
  const memories = activity.assets.find((asset) => asset.kind === "memories");
  const embed = memories?.steps.find((step) => step.name === "embed");
  if (embed === undefined || embed.state !== "running") return 0;
  return Math.max(0, embed.total - embed.done);
}

/** "3 of 12 memories": a step's count with the unit it actually counts. */
export function stepCount(step: KnownActivityStep): Phrase {
  return {
    key: `activityStatus.stepCount.${STEP_UNIT[step.name]}`,
    params: { count: step.total, done: step.done },
  };
}

/** The row's number, always in the asset's own unit. */
export function assetCount(asset: KnownActivityAsset): number {
  return findStep(asset, OWN_STEP[asset.kind])?.total ?? asset.total;
}

/**
 * The row's progress, as a fraction of the work Wenlan does on its own.
 *
 * Entities draws Detect (memories scanned), not Confirm: confirming is the
 * user's call in the Wiki, so a bar that filled only as the user confirmed
 * would read as background work that never finishes.
 */
export function assetProgress(asset: KnownActivityAsset): number {
  const { done, total } =
    asset.kind === "entities" ? (findStep(asset, "detect") ?? asset) : asset;
  // An asset with nothing in it has no progress to draw; 0/0 would otherwise
  // render as a full bar, which reads as finished rather than empty.
  return total === 0 ? 0 : done / total;
}

/**
 * Which sentence an asset row gets.
 *
 * The two Blocked causes are told apart by the governing route, not by the
 * counts: an unavailable lane means the work never started (no model loaded),
 * an available one means the work ran and failed. The spec requires both to
 * name their own cause and next action, and only the route can tell them
 * apart — `blocked > 0` is true in both cases.
 *
 * Every number is in the unit its sentence names. Entities' blocked and
 * running counts are memories (the daemon counts memories not yet scanned),
 * so their copy says "memories"; only the settled sentence counts entities.
 */
export function assetSentence(
  activity: ActivityResponse,
  asset: KnownActivityAsset,
): Phrase {
  const kind = asset.kind;
  if (activity.state === "off") return onDemandSentence(asset);
  const route = routeFor(activity, kind);

  if (asset.blocked > 0) {
    const scanned = kind === "entities" ? findStep(asset, "detect") : undefined;
    return route.available
      ? {
          key: `activityStatus.assetBlockedFailed.${kind}`,
          params: { count: asset.blocked, total: (scanned ?? asset).total },
        }
      : {
          key: `activityStatus.assetBlockedNoModel.${kind}`,
          params: { count: asset.blocked },
        };
  }

  if (kind === "entities") return entitySentence(asset);

  if (asset.total === 0) return { key: `activityStatus.assetEmpty.${kind}` };

  if (asset.done < asset.total) {
    return {
      key: `activityStatus.assetRunning.${kind}`,
      params: { done: asset.done, total: asset.total },
    };
  }

  return {
    key: `activityStatus.assetDone.${kind}`,
    params: { count: asset.total },
  };
}

/**
 * A row in On demand mode (`state: off`). Nothing runs in the background, so
 * pending work is not "running" or "blocked": it waits for the user's AI tool.
 * Each row counts what is still waiting, in its own unit, and falls back to
 * the settled sentence once nothing is.
 */
function onDemandSentence(asset: KnownActivityAsset): Phrase {
  const kind = asset.kind;
  if (kind === "memories") {
    const stored = assetCount(asset);
    return stored === 0
      ? { key: "activityStatus.assetEmpty.memories" }
      : { key: "activityStatus.assetOnDemand.memories", params: { count: stored } };
  }
  const waiting = onDemandBacklog(asset);
  if (waiting > 0) {
    return { key: `activityStatus.assetOnDemand.${kind}`, params: { count: waiting } };
  }
  if (kind === "entities") return entitySentence(asset);
  return asset.total === 0
    ? { key: "activityStatus.assetEmpty.pages" }
    : { key: "activityStatus.assetDone.pages", params: { count: asset.total } };
}

/** Entities: memories not yet scanned. Pages: pages not yet current. */
function onDemandBacklog(asset: KnownActivityAsset): number {
  if (asset.kind === "memories") return 0;
  const { done, total } = asset.kind === "entities" ? (findStep(asset, "detect") ?? asset) : asset;
  return Math.max(0, total - done);
}

/**
 * How many pages wait for the user's AI tool in On demand mode. Zero in every
 * other state, where that work belongs to the background.
 */
export function onDemandPagesWaiting(activity: ActivityResponse): number {
  if (activity.state !== "off") return 0;
  const pages = knownAssets(activity).find((asset) => asset.kind === "pages");
  return pages === undefined ? 0 : onDemandBacklog(pages);
}

/**
 * Entities is running only while memories are still being scanned. Confirm is
 * never "running": an entity waits for the user in the Wiki, so found but
 * unconfirmed entities are the settled state, not unfinished work.
 */
function entitySentence(asset: KnownActivityAsset): Phrase {
  const detect = findStep(asset, "detect");
  if (detect !== undefined && detect.done < detect.total) {
    return {
      key: "activityStatus.assetRunning.entities",
      params: { count: detect.total, done: detect.done, total: detect.total },
    };
  }

  const found = assetCount(asset);
  if (found === 0) return { key: "activityStatus.assetEmpty.entities" };

  // "21 found, 9 confirmed in the Wiki": an entity can be detected and never
  // earn enough substance to be confirmed.
  return {
    key: "activityStatus.assetDone.entities",
    params: { count: found, confirmed: findStep(asset, "confirm")?.done ?? 0 },
  };
}

/**
 * The missing models behind a Blocked library, each named once.
 *
 * A row only says how many items are waiting; the cause and the fix belong to
 * the model, not the asset. Memories and Entities both run on the everyday
 * model, so a per-row cause repeated the same sentence. Failed work (the lane
 * is available) is not listed: its rows carry their own cause.
 */
export function blockedCauses(activity: ActivityResponse): Phrase[] {
  if (activity.state === "off") return [];
  const jobs = new Set<ActivityRouteJob>();
  // An asset this build cannot name is skipped: its governing model is
  // unknown here, and a guessed cause would send the user to the wrong fix.
  for (const asset of knownAssets(activity)) {
    if (asset.blocked > 0 && !routeFor(activity, asset.kind).available) {
      jobs.add(governingJob(asset.kind));
    }
  }
  // A pinned source that is not serving (a model still loading, a local
  // server that is down) is a different fix from no choice at all: saying
  // "none is chosen" there would contradict the Intelligence page.
  return ROUTE_JOBS
    .filter((job) => jobs.has(job))
    .map((job) => ({
      key:
        activity[job].mode === "pinned_unavailable"
          ? `activityStatus.blockedCauseUnavailable.${job}`
          : `activityStatus.blockedCause.${job}`,
    }));
}

/** One Suggestions line: which count it is, and its sentence. */
export interface SuggestionLine extends Phrase {
  readonly kind: "ready" | "not-ready";
}

/**
 * The Suggestions lines, one per non-zero count, ready first. Empty when no
 * suggestion is open, and the section hides. These counts never feed the
 * state: some open suggestions never move on their own.
 */
export function suggestionLines(activity: ActivityResponse): SuggestionLine[] {
  const { ready_for_review: ready, not_ready: notReady } = activity.refinement;
  const lines: SuggestionLine[] = [];
  if (ready > 0) {
    lines.push({
      kind: "ready",
      key: "activityStatus.suggestions.ready",
      params: { count: ready },
    });
  }
  if (notReady > 0) {
    lines.push({
      kind: "not-ready",
      key: "activityStatus.suggestions.notReady",
      params: { count: notReady },
    });
  }
  return lines;
}

/** The "Everyday work: qwen3-8b on this machine." sentence for one route. */
export function routeSentence(route: ActivityRoute): Phrase {
  return route.model === null
    ? { key: "activityStatus.routeNoModel" }
    : { key: "activityStatus.route", params: { model: route.model } };
}

/**
 * Whether each lane sends text to a vendor. A table rather than a list of the
 * cloud ones, so a lane added to the wire type fails `tsc` here until someone
 * decides which side it is on.
 */
const LANE_IS_CLOUD: Readonly<Record<KnownActivityLane, boolean>> = {
  on_device: false,
  external: false,
  anthropic: true,
  basic: false,
  none: false,
};

/** The lane, or undefined when a newer daemon named one this app predates. */
export function knownLane(lane: ActivityLane): KnownActivityLane | undefined {
  return has(LANE_IS_CLOUD, lane) ? lane : undefined;
}

/** Lane chip copy for a resolved route. */
export function laneKey(lane: KnownActivityLane): ParseKeys {
  return `activityStatus.lane.${lane}`;
}

export function isCloud(lane: KnownActivityLane): boolean {
  return LANE_IS_CLOUD[lane];
}

/**
 * The trust sentence, as keys rather than a Phrase.
 *
 * The cloud case interpolates a LIST of job names, and a list of keys cannot
 * ride inside `params` without being flattened to a string and losing its
 * types. So it travels as an array and the component joins it with the
 * locale's own separator.
 */
export type TrustPhrase =
  | { readonly kind: "local"; readonly key: "activityStatus.trustLocal" }
  | {
      readonly kind: "cloud";
      readonly key: "activityStatus.trustCloud";
      readonly jobKeys: readonly ParseKeys[];
      readonly vendorKey: ParseKeys;
    };

/**
 * "Nothing leaves your device" is claimed only when NO resolved lane is a
 * cloud vendor. `external` is a local server the user runs, so it stays local;
 * `basic` and `none` run no model at all. Any cloud lane names its vendor and
 * says which work leaves the device, because claiming local while sending text
 * to a vendor is the one thing this sentence must never do.
 *
 * So a lane this build does not know gets no sentence at all (undefined): it
 * may be a vendor this app has never heard of, and "nothing leaves your
 * device" would then be the false claim.
 *
 * Job names come from the field carrying each route, not from `route.job`,
 * which a newer daemon could fill with a word this build cannot name.
 */
export function trustSentence(activity: ActivityResponse): TrustPhrase | undefined {
  const cloud: { job: ActivityRouteJob; lane: KnownActivityLane }[] = [];
  for (const job of ROUTE_JOBS) {
    const lane = knownLane(activity[job].lane);
    if (lane === undefined) return undefined;
    if (isCloud(lane)) cloud.push({ job, lane });
  }
  if (cloud.length === 0)
    return { kind: "local", key: "activityStatus.trustLocal" };

  return {
    kind: "cloud",
    key: "activityStatus.trustCloud",
    jobKeys: cloud.map(({ job }) => `activityStatus.job.${job}` as const),
    vendorKey: laneKey(cloud[0].lane),
  };
}
