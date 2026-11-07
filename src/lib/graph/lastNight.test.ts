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
    expect(nightCounts(things, (id) => on.has(id))).toEqual({ written: 2, connected: 1 });
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
      { thing: thingNodeId("a"), grow: [memoryNodeId("m1"), memoryNodeId("m2")], fly: [memoryNodeId("m1"), memoryNodeId("m2")], topics: ["e1"] },
    ]);
  });

  it("stops at five things at most", () => {
    const things = Array.from({ length: 8 }, (_, i) => thing(`t${i}`, { at: at(3), written: [`m${i}`] }));
    expect(replayStops(things, () => true)).toHaveLength(REPLAY_MAX_THINGS);
  });

  it("keeps a long night's growing short", () => {
    expect(growStagger(1)).toBe(0);
    expect(growStagger(8)).toBe(80);
    expect(growMs(8)).toBe(400 + 7 * 80);
    expect(growMs(31)).toBe(400 + 600);
  });

  it("plays 8 points in 2 things with links in under ten seconds", () => {
    const things = [
      thing("a", { at: at(3), written: ["a1", "a2", "a3", "a4", "a5"], linked: [["a1", "e1"], ["a2", "e2"], ["a3", "e2"]] }),
      thing("b", { at: at(3), written: ["b1", "b2", "b3"], linked: [["b1", "e3"], ["b2", "e3"]] }),
    ];
    const ms = replayMs(replayStops(things, () => true));
    expect(ms).toBeGreaterThan(6_000);
    expect(ms).toBeLessThan(10_000);
    expect(replayMs([])).toBe(0);
  });
});
