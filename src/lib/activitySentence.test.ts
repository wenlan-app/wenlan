// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { i18n } from "../i18n";
import {
  assetCount,
  assetProgress,
  assetSentence,
  blockedCauses,
  governingJob,
  isCloud,
  knownAssets,
  knownLane,
  knownState,
  laneKey,
  routeFor,
  routeSentence,
  stepCount,
  trustSentence,
  type KnownActivityAsset,
  type KnownActivityAssetKind,
  type KnownActivityLane,
  type KnownActivityStep,
} from "./activitySentence";
import type {
  ActivityAssetStatus,
  ActivityLane,
  ActivityResponse,
  ActivityRoute,
} from "./tauri";

function route(
  job: "everyday" | "synthesis",
  lane: ActivityLane,
  available: boolean,
): ActivityRoute {
  return {
    job,
    lane,
    model: available ? "a-model" : null,
    mode: available ? "pinned" : "unconfigured",
    available,
  };
}

function asset(
  kind: KnownActivityAssetKind,
  fields: Partial<KnownActivityAsset> = {},
): KnownActivityAsset {
  return {
    kind,
    state: "idle",
    done: 0,
    total: 0,
    blocked: 0,
    steps: [],
    ...fields,
  };
}

function step(
  name: KnownActivityStep["name"],
  done: number,
  total: number = done,
): KnownActivityStep {
  return { name, state: "idle", done, total, failed: 0, job: null };
}

function activity(fields: Partial<ActivityResponse> = {}): ActivityResponse {
  return {
    state: "up_to_date",
    last_activity_at: null,
    assets: [],
    everyday: route("everyday", "on_device", true),
    synthesis: route("synthesis", "on_device", true),
    refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
    ...fields,
  };
}

describe("knownState", () => {
  it("passes every state this build has words for through unchanged", () => {
    for (const state of ["up_to_date", "off", "organizing", "waiting_for_idle", "blocked"] as const) {
      expect(knownState(activity({ state }))).toBe(state);
    }
  });

  it("claims nothing for a state from a newer daemon", () => {
    expect(knownState(activity({ state: "unknown" }))).toBeUndefined();
  });
});

describe("governingJob / routeFor", () => {
  it("routes pages through synthesis and everything else through everyday", () => {
    expect(governingJob("pages")).toBe("synthesis");
    expect(governingJob("memories")).toBe("everyday");
    expect(governingJob("entities")).toBe("everyday");

    const a = activity({
      everyday: route("everyday", "on_device", true),
      synthesis: route("synthesis", "anthropic", true),
    });
    expect(routeFor(a, "pages").lane).toBe("anthropic");
    expect(routeFor(a, "memories").lane).toBe("on_device");
  });
});

describe("blockedCauses", () => {
  it("does not present a missing model as a fault when background AI is off", () => {
    const a = activity({
      state: "off",
      everyday: route("everyday", "none", false),
      assets: [asset("memories", { total: 5, blocked: 5 })],
    });
    expect(blockedCauses(a)).toEqual([]);
    expect(assetSentence(a, asset("memories", { total: 5, blocked: 5 }))).toEqual({
      key: "activityStatus.assetOff.memories",
    });
  });

  it("lists each missing model once, in job order", () => {
    const a = activity({
      everyday: route("everyday", "none", false),
      synthesis: route("synthesis", "none", false),
      assets: [
        asset("pages", { blocked: 1, total: 1 }),
        asset("memories", { blocked: 8, total: 8 }),
        asset("entities", { blocked: 8, total: 8 }),
      ],
    });
    expect(blockedCauses(a)).toEqual([
      { key: "activityStatus.blockedCause.everyday" },
      { key: "activityStatus.blockedCause.synthesis" },
    ]);
  });

  it("says a chosen model is unavailable rather than unchosen", () => {
    const a = activity({
      everyday: { ...route("everyday", "basic", false), mode: "pinned_unavailable" },
      synthesis: route("synthesis", "none", false),
      assets: [
        asset("memories", { blocked: 3, total: 3 }),
        asset("pages", { blocked: 1, total: 1 }),
      ],
    });
    expect(blockedCauses(a)).toEqual([
      { key: "activityStatus.blockedCauseUnavailable.everyday" },
      { key: "activityStatus.blockedCause.synthesis" },
    ]);
  });

  it("ignores failed work and unavailable models with nothing blocked", () => {
    const a = activity({
      synthesis: route("synthesis", "none", false),
      assets: [asset("memories", { blocked: 2, total: 5 }), asset("pages")],
    });
    // Memories failed on an available model; Pages is blocked on nothing.
    expect(blockedCauses(a)).toEqual([]);
  });
});

describe("routeSentence", () => {
  it("prints the lane alone when no model is loaded", () => {
    const phrase = routeSentence(route("everyday", "basic", false));
    expect(
      i18n.t(phrase.key, {
        ...phrase.params,
        job: i18n.t("activityStatus.jobTitle.everyday"),
        lane: i18n.t(laneKey("basic")),
      }),
    ).toBe("Everyday work: built in, no model.");
  });

  it("names the model and its lane when one is loaded", () => {
    const phrase = routeSentence(route("synthesis", "on_device", true));
    expect(
      i18n.t(phrase.key, {
        ...phrase.params,
        job: i18n.t("activityStatus.jobTitle.synthesis"),
        lane: i18n.t(laneKey("on_device")),
      }),
    ).toBe("Page writing: a-model on this machine.");
  });
});

describe("assetSentence", () => {
  it("tells the two Blocked causes apart by the route, not the counts", () => {
    // Identical asset, opposite routes: the only difference is whether a model
    // is loaded, which is exactly what the user needs told apart.
    const stuck = asset("memories", { blocked: 3, total: 10, done: 7 });

    expect(assetSentence(activity(), stuck)).toEqual({
      key: "activityStatus.assetBlockedFailed.memories",
      params: { count: 3, total: 10 },
    });

    const noModel = activity({
      everyday: route("everyday", "none", false),
    });
    expect(assetSentence(noModel, stuck)).toEqual({
      key: "activityStatus.assetBlockedNoModel.memories",
      params: { count: 3 },
    });
  });

  it("reads the pages route, not the everyday route, for a blocked page", () => {
    const a = activity({
      everyday: route("everyday", "on_device", true),
      synthesis: route("synthesis", "none", false),
    });
    expect(assetSentence(a, asset("pages", { blocked: 2, total: 2 })).key).toBe(
      "activityStatus.assetBlockedNoModel.pages",
    );
  });

  it("prefers Blocked over every other sentence", () => {
    // done === total and blocked > 0 at once: the finished-looking counts must
    // not win, or a stuck asset would read as done.
    const both = asset("entities", { blocked: 1, total: 4, done: 4 });
    expect(assetSentence(activity(), both).key).toBe(
      "activityStatus.assetBlockedFailed.entities",
    );
  });

  it("says empty when nothing has arrived", () => {
    expect(assetSentence(activity(), asset("pages"))).toEqual({
      key: "activityStatus.assetEmpty.pages",
    });
  });

  it("says running while work is outstanding", () => {
    expect(
      assetSentence(activity(), asset("memories", { done: 4, total: 9 })),
    ).toEqual({
      key: "activityStatus.assetRunning.memories",
      params: { done: 4, total: 9 },
    });
  });

  it("takes the entity confirmed count from the confirm step", () => {
    const entities = asset("entities", {
      done: 9,
      total: 21,
      steps: [step("detect", 40), step("confirm", 9, 21)],
    });
    expect(assetSentence(activity(), entities)).toEqual({
      key: "activityStatus.assetDone.entities",
      params: { count: 21, confirmed: 9 },
    });
  });

  it("reports zero confirmed when the confirm step is absent", () => {
    const entities = asset("entities", { done: 3, total: 3 });
    expect(assetSentence(activity(), entities).params).toEqual({
      count: 3,
      confirmed: 0,
    });
  });
});

describe("units", () => {
  // The Entities asset's done/total follow its busy step, and Detect counts
  // memories while Confirm counts entities. These pin every number to the unit
  // its sentence names: a memory count beside "Entities" is a wrong number.
  const scanning = asset("entities", {
    done: 2,
    total: 10,
    steps: [step("detect", 2, 10), step("confirm", 1, 3)],
  });

  it("counts an entities row in entities even while memories are scanned", () => {
    expect(assetCount(scanning)).toBe(3);
    expect(assetCount(asset("memories", { total: 7, steps: [step("store", 9)] }))).toBe(9);
    expect(assetCount(asset("pages", { total: 4 }))).toBe(4);
  });

  it("says scanning in memories, with the memory total as the plural count", () => {
    expect(assetSentence(activity(), scanning)).toEqual({
      key: "activityStatus.assetRunning.entities",
      params: { count: 10, done: 2, total: 10 },
    });
    expect(i18n.t("activityStatus.assetRunning.entities", { count: 10, done: 2, total: 10 })).toBe(
      "2 of 10 memories scanned for entities",
    );
  });

  it("treats unconfirmed entities as settled, not as unfinished work", () => {
    // Confirm waits for the user in the Wiki; 3 found and 1 confirmed is done.
    const settled = asset("entities", {
      done: 1,
      total: 3,
      steps: [step("detect", 10), step("confirm", 1, 3)],
    });
    expect(assetSentence(activity(), settled)).toEqual({
      key: "activityStatus.assetDone.entities",
      params: { count: 3, confirmed: 1 },
    });
    expect(assetProgress(settled)).toBe(1);
  });

  it("names memories when entity detection is blocked", () => {
    const noModel = activity({ everyday: route("everyday", "none", false) });
    const blocked = asset("entities", {
      blocked: 8,
      done: 0,
      total: 8,
      steps: [step("detect", 0, 8), step("confirm", 0, 0)],
    });
    const phrase = assetSentence(noModel, blocked);
    expect(i18n.t(phrase.key, phrase.params)).toBe(
      "8 memories not yet scanned for entities",
    );

    const failed = assetSentence(activity(), {
      ...blocked,
      blocked: 2,
      steps: [step("detect", 6, 8), step("confirm", 4, 5)],
    });
    // total is the memory population, not the 5 entities found.
    expect(failed.params).toEqual({ count: 2, total: 8 });
  });

  it("keeps the searchable reassurance on blocked memories", () => {
    const noModel = activity({ everyday: route("everyday", "none", false) });
    const phrase = assetSentence(noModel, asset("memories", { blocked: 8, total: 8 }));
    expect(i18n.t(phrase.key, phrase.params)).toBe(
      "8 not yet summarized. All are searchable now.",
    );
  });

  it("counts each step in the unit it works on", () => {
    const say = (s: KnownActivityStep) => {
      const phrase = stepCount(s);
      return i18n.t(phrase.key, phrase.params);
    };
    expect(say(step("detect", 2, 10))).toBe("2 of 10 memories");
    expect(say(step("confirm", 1, 3))).toBe("1 of 3 entities");
    expect(say(step("write", 1, 1))).toBe("1 of 1 page");
  });
});

describe("trustSentence", () => {
  it("claims local only when no resolved lane is a cloud vendor", () => {
    const lanes: ActivityLane[] = ["on_device", "external", "basic", "none"];
    for (const lane of lanes) {
      const a = activity({
        everyday: route("everyday", lane, lane !== "none"),
        synthesis: route("synthesis", lane, lane !== "none"),
      });
      expect(trustSentence(a)).toEqual({
        kind: "local",
        key: "activityStatus.trustLocal",
      });
    }
  });

  it("names the vendor and the job when one lane is cloud", () => {
    const a = activity({
      everyday: route("everyday", "on_device", true),
      synthesis: route("synthesis", "anthropic", true),
    });
    expect(trustSentence(a)).toEqual({
      kind: "cloud",
      key: "activityStatus.trustCloud",
      jobKeys: ["activityStatus.job.synthesis"],
      vendorKey: "activityStatus.lane.anthropic",
    });
  });

  it("lists both jobs when both run in the cloud", () => {
    const a = activity({
      everyday: route("everyday", "anthropic", true),
      synthesis: route("synthesis", "anthropic", true),
    });
    const trust = trustSentence(a);
    expect(trust?.kind === "cloud" && trust.jobKeys).toEqual([
      "activityStatus.job.everyday",
      "activityStatus.job.synthesis",
    ]);
  });

  it("treats a local server as local, not as a vendor", () => {
    expect(isCloud("external")).toBe(false);
    expect(isCloud("anthropic")).toBe(true);
  });

  // A lane this app predates may be a vendor it has never heard of, so it must
  // never read as "nothing leaves your device".
  it("says nothing about trust when a lane is unknown", () => {
    // Beside a cloud lane too: naming only Anthropic would imply the unknown
    // lane stays on the device.
    for (const other of ["on_device", "anthropic"] as const) {
      for (const lane of ["unknown", "a_lane_from_a_newer_daemon"]) {
        const a = activity({
          everyday: route("everyday", other, true),
          synthesis: route("synthesis", lane as ActivityLane, true),
        });
        expect(trustSentence(a)).toBeUndefined();
      }
    }
  });

  it("names jobs by the field that carries each route, not by route.job", () => {
    const a = activity({
      everyday: { ...route("everyday", "anthropic", true), job: "unknown" },
    });
    const trust = trustSentence(a);
    expect(trust?.kind === "cloud" && trust.jobKeys).toEqual(["activityStatus.job.everyday"]);
  });
});

describe("words from a newer daemon", () => {
  it("knows each lane this build has copy for, and no other", () => {
    expect(knownLane("on_device")).toBe("on_device");
    expect(knownLane("unknown")).toBeUndefined();
    expect(knownLane("a_lane_from_a_newer_daemon" as ActivityLane)).toBeUndefined();
  });

  it("reads a raw new state word as unknown, as the browser preview sends it", () => {
    expect(knownState(activity({ state: "paused" as ActivityResponse["state"] }))).toBeUndefined();
  });

  it("leaves out assets and steps this build cannot name", () => {
    const newer: ActivityAssetStatus = {
      ...asset("memories"),
      kind: "unknown",
      blocked: 5,
    };
    const memories: ActivityAssetStatus = {
      ...asset("memories", { total: 4 }),
      steps: [
        step("store", 4),
        { ...step("store", 1), name: "unknown" },
        { ...step("store", 1), name: "a_step_from_a_newer_daemon" as "unknown" },
      ],
    };
    const known = knownAssets(activity({ assets: [newer, memories] }));
    expect(known.map((a) => a.kind)).toEqual(["memories"]);
    expect(known[0].steps.map((s) => s.name)).toEqual(["store"]);
    expect(known[0].total).toBe(4);
  });

  it("never names a missing model for an asset it cannot name", () => {
    const newer: ActivityAssetStatus = { ...asset("memories", { blocked: 5 }), kind: "unknown" };
    const a = activity({
      assets: [newer],
      everyday: route("everyday", "none", false),
      synthesis: route("synthesis", "none", false),
    });
    expect(blockedCauses(a)).toEqual([]);
  });
});

describe("key coverage", () => {
  // Every key these functions can return must exist, or the surface renders the
  // key string itself. i18n parity is covered elsewhere; this checks that the
  // mapping and the copy agree on names.
  const kinds: KnownActivityAssetKind[] = ["memories", "entities", "pages"];
  const lanes: KnownActivityLane[] = [
    "on_device",
    "external",
    "anthropic",
    "basic",
    "none",
  ];

  it("resolves every asset sentence key", () => {
    const shapes: KnownActivityAsset[] = kinds.flatMap((kind) => [
      asset(kind),
      asset(kind, { done: 1, total: 4 }),
      asset(kind, { done: 4, total: 4 }),
      asset(kind, { done: 1, total: 4, blocked: 3 }),
      asset(kind, {
        done: 1,
        total: 4,
        steps: [step("detect", 1, 4), step("confirm", 0, 2)],
      }),
    ]);
    for (const available of [true, false]) {
      const a = activity({
        everyday: route("everyday", available ? "on_device" : "none", available),
        synthesis: route(
          "synthesis",
          available ? "on_device" : "none",
          available,
        ),
      });
      for (const shape of shapes) {
        const phrase = assetSentence(a, shape);
        expect(i18n.exists(phrase.key, phrase.params)).toBe(true);
      }
    }
  });

  it("resolves every lane key", () => {
    for (const lane of lanes) expect(i18n.exists(laneKey(lane))).toBe(true);
  });
});
