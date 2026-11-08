// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { ThingNightPoints } from "../tauri";
import { memoryNodeId, thingNodeId } from "./model";
import {
  growMs,
  growStagger,
  nightCounts,
  nightsToday,
  ranOvernight,
  REPLAY_MAX_THINGS,
  replayMs,
  replayStops,
  tidyMs,
  todayStartSeconds,
} from "./lastNight";

const NOW = new Date(2026, 10, 7, 9, 30);
const at = (hour: number, dayOffset = 0) => Math.floor(new Date(2026, 10, 7 + dayOffset, hour).getTime() / 1000);

function thing(id: string, night?: { at: number; written: string[]; linked?: [string, string][] }, spaces = ["wenlan"]): ThingNightPoints {
  return {
    thing_id: id,
    title: id,
    spaces,
    points: (night?.written ?? []).map((memory) => ({ memory_id: memory, title: memory, decided: true, created_at: night!.at })),
    ...(night ? { last_night: { at: night.at, written: night.written, linked: (night.linked ?? []).map(([memory_id, entity_id]) => ({ memory_id, entity_id })) } } : {}),
  };
}

describe("last night on the big map (UX-GRAPH-5)", () => {
  it("keeps only things whose newest night ran today, in the Space looked at", () => {
    const things = [
      thing("a", { at: at(3), written: ["m1"] }),
      thing("b", { at: at(3, -1), written: ["m2"] }),
      thing("c", { at: at(4), written: ["m3"] }, ["other"]),
      thing("d"),
    ];
    expect(nightsToday(things, null, NOW).map((t) => t.thing_id)).toEqual(["a", "c"]);
    expect(nightsToday(things, "wenlan", NOW).map((t) => t.thing_id)).toEqual(["a"]);
  });

  it("says last night only when every night ran before 7 in the morning", () => {
    expect(ranOvernight([at(2), at(6)])).toBe(true);
    expect(ranOvernight([at(2), at(8)])).toBe(false);
  });

  it("counts what is on the map now, and a point connected twice once", () => {
    const things = [thing("a", { at: at(3), written: ["m1", "m2", "gone"], linked: [["m1", "e1"], ["m1", "e2"], ["m2", "elsewhere"]] })];
    const on = new Set([memoryNodeId("m1"), memoryNodeId("m2"), "e1", "e2"]);
    expect(nightCounts(things, (id) => on.has(id))).toEqual({ written: 2, connected: 1, merged: 0, replaced: 0, updated: 0 });
  });

  it("counts what a night tidied (UX-GRAPH-7) while what stands now is on the map", () => {
    const tidied = thing("a", { at: at(3), written: [] });
    tidied.last_night = {
      at: at(3),
      written: [],
      linked: [],
      tidied: [
        { kind: "merge", memory_id: "m1", replaced: ["o1", "o2"] },
        { kind: "replace", memory_id: "kept", replaced: ["o3"] },
        { kind: "update", memory_id: "m2", replaced: ["o4"] },
        { kind: "update", memory_id: "gone", replaced: ["o5"] },
      ],
    };
    const on = new Set([thingNodeId("a"), memoryNodeId("m1"), memoryNodeId("m2")]);
    expect(nightCounts([tidied], (id) => on.has(id))).toEqual({ written: 0, connected: 0, merged: 1, replaced: 1, updated: 1 });
    // What a merge or update wrote grows in Replay like a new point.
    expect(replayStops([tidied], (id) => on.has(id))[0]!.grow).toEqual([memoryNodeId("m1"), memoryNodeId("m2")]);
  });

  it("takes back from local midnight", () => {
    expect(todayStartSeconds(NOW)).toBe(Math.floor(new Date(2026, 10, 7).getTime() / 1000));
  });
});

describe("Replay's stops (UX-GRAPH-6)", () => {
  it("is one stop per thing on the map, with what grows and what flies", () => {
    const things = [
      thing("a", { at: at(3), written: ["m1", "m2"], linked: [["m1", "e1"], ["m2", "e1"]] }),
      thing("off-map", { at: at(3), written: ["m9"] }),
    ];
    const on = new Set([thingNodeId("a"), memoryNodeId("m1"), memoryNodeId("m2"), "e1", memoryNodeId("m9")]);
    expect(replayStops(things, (id) => on.has(id))).toEqual([
      { thing: thingNodeId("a"), grow: [memoryNodeId("m1"), memoryNodeId("m2")], tidies: [], fly: [memoryNodeId("m1"), memoryNodeId("m2")], topics: ["e1"] },
    ]);
  });

  it("shows each tidy (UX-GRAPH-7) by what stands now, and a replace into the thing when that is off the map", () => {
    const tidied = thing("a", { at: at(3), written: [] });
    tidied.last_night = {
      at: at(3),
      written: [],
      linked: [],
      tidied: [
        { kind: "merge", memory_id: "m1", replaced: ["o1", "o2"] },
        { kind: "merge", memory_id: "m3", replaced: ["o1", "o2", "o3", "o4", "o5"] },
        { kind: "replace", memory_id: "elsewhere", replaced: ["o6"] },
        { kind: "update", memory_id: "gone", replaced: ["o7"] },
      ],
    };
    const on = new Set([thingNodeId("a"), memoryNodeId("m1"), memoryNodeId("m3")]);
    const [stop] = replayStops([tidied], (id) => on.has(id));
    expect(stop!.tidies).toEqual([
      { kind: "merge", survivor: memoryNodeId("m1"), ghosts: 2 },
      { kind: "merge", survivor: memoryNodeId("m3"), ghosts: 3 },
      { kind: "replace", survivor: null, ghosts: 1 },
    ]);
    // A night that only replaced is still a stop.
    const replacedOnly = thing("b", { at: at(3), written: [] });
    replacedOnly.last_night = { at: at(3), written: [], linked: [], tidied: [{ kind: "replace", memory_id: "kept", replaced: ["o8"] }] };
    expect(replayStops([replacedOnly], (id) => id === thingNodeId("b"))).toHaveLength(1);
  });

  it("takes about a second more for a stop that tidied", () => {
    expect(tidyMs(0)).toBe(0);
    expect(tidyMs(1)).toBe(200 + 450);
    expect(tidyMs(3)).toBe(200 + 450 + 2 * 110);
    const plain = thing("a", { at: at(3), written: ["a1"] });
    const tidied = thing("a", { at: at(3), written: ["a1"] });
    tidied.last_night!.tidied = [{ kind: "update", memory_id: "a1", replaced: ["o1"] }];
    const extra = replayMs(replayStops([tidied], () => true)) - replayMs(replayStops([plain], () => true));
    expect(extra).toBe(200 + 450 + 450);
  });

  it("stops at five things at most", () => {
    const things = Array.from({ length: 8 }, (_, i) => thing(`t${i}`, { at: at(3), written: [`m${i}`] }));
    expect(replayStops(things, () => true)).toHaveLength(REPLAY_MAX_THINGS);
  });

  it("keeps a long night's growing short", () => {
    expect(growStagger(1)).toBe(0);
    expect(growStagger(8)).toBe(110);
    expect(growMs(8)).toBe(550 + 7 * 110);
    expect(growMs(31)).toBe(550 + 800);
  });

  it("plays 8 points in 2 things with links in under thirteen seconds", () => {
    const things = [
      thing("a", { at: at(3), written: ["a1", "a2", "a3", "a4", "a5"], linked: [["a1", "e1"], ["a2", "e2"], ["a3", "e2"]] }),
      thing("b", { at: at(3), written: ["b1", "b2", "b3"], linked: [["b1", "e3"], ["b2", "e3"]] }),
    ];
    const ms = replayMs(replayStops(things, () => true));
    expect(ms).toBeGreaterThan(8_000);
    expect(ms).toBeLessThan(13_000);
    expect(replayMs([])).toBe(0);
  });
});
