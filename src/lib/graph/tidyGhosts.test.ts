// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { GHOST_ALPHA, ghostAt, ghostPhase, ghostRests, ghostsMoving, saidLine, swellOf, type ReplayGhost } from "./tidyGhosts";

const merge = (startAt = 0): ReplayGhost => ({ tidy: { kind: "merge", survivor: "m1", ghosts: 2, said: [] }, thing: "t", startAt, still: false });
const update: ReplayGhost = { tidy: { kind: "update", survivor: "m2", ghosts: 1, said: [] }, thing: "t", startAt: 0, still: false };
const replace: ReplayGhost = { tidy: { kind: "replace", survivor: "m3", ghosts: 1, said: [] }, thing: "t", startAt: 0, still: false };
const intoThing: ReplayGhost = { tidy: { kind: "replace", survivor: null, ghosts: 1, said: [] }, thing: "t", startAt: 0, still: false };

describe("Replay's tidy ghosts (UX-GRAPH-7)", () => {
  it("rests a merge's two ghosts on the far side from the thing, splayed, two dot-widths out", () => {
    const [a, b] = ghostRests({ x: 100, y: 0 }, { x: 0, y: 0 }, 5, 2);
    for (const rest of [a!, b!]) {
      expect(Math.hypot(rest.x - 100, rest.y)).toBeCloseTo(20);
      expect(rest.x).toBeGreaterThan(100);
    }
    expect(Math.atan2(a!.y, a!.x - 100) * (180 / Math.PI)).toBeCloseTo(-35);
    expect(Math.atan2(b!.y, b!.x - 100) * (180 / Math.PI)).toBeCloseTo(35);
    // Off the map: beside the thing's centre.
    expect(ghostRests(null, { x: 0, y: 0 }, 5, 1)).toHaveLength(1);
    expect(ghostRests(null, null, 5, 1)).toEqual([]);
  });

  it("rests with the words that went, long enough to read, before it moves", () => {
    expect(ghostPhase(merge(), 600)).toMatchObject({ alpha: GHOST_ALPHA, words: 1, slide: 0 });
    expect(ghostPhase(merge(), 1100 + 300).words).toBe(0);
  });

  it("slides a merge's ghosts in, swells once, and is gone", () => {
    expect(ghostPhase(merge(), -10).alpha).toBe(0);
    expect(ghostPhase(merge(), 100).alpha).toBeCloseTo(GHOST_ALPHA / 2);
    const sliding = ghostPhase(merge(), 1100 + 300);
    expect(sliding.alpha).toBe(GHOST_ALPHA);
    expect(sliding.slide).toBeCloseTo(0.5);
    const swelling = ghostPhase(merge(), 1700 + 75);
    expect(swelling.alpha).toBe(0);
    expect(swelling.swell).toBeCloseTo(1.25);
    expect(ghostPhase(merge(), 1700 + 150).done).toBe(true);
    expect(ghostAt({ x: 0, y: 0 }, { x: 10, y: 20 }, 0.5)).toEqual({ x: 5, y: 10 });
  });

  it("knocks a replace's ghost back off the dot, shrinking as it fades, with no swell", () => {
    const coming = ghostPhase(replace, 1100 + 200);
    expect(coming.slide).toBeGreaterThan(0);
    expect(coming.alpha).toBe(GHOST_ALPHA);
    const pushed = ghostPhase(replace, 1100 + 500);
    expect(pushed.slide).toBeLessThan(0);
    expect(pushed.scale).toBeLessThan(1);
    expect(pushed.alpha).toBeLessThan(GHOST_ALPHA);
    expect(pushed.swell).toBe(1);
    expect(ghostPhase(replace, 1700 + 1)).toMatchObject({ alpha: 0, swell: 1, done: true });
    expect(swellOf([replace], "m3", 1700 + 75)).toBe(1);
    expect(ghostPhase(intoThing, 1700 + 1).done).toBe(true);
  });

  it("spreads an update's old version out from the dot itself, then swells it once", () => {
    expect(ghostPhase(update, 100).slide).toBe(1);
    expect(ghostPhase(update, 100).scale).toBeGreaterThan(1);
    const spreading = ghostPhase(update, 1100 + 300);
    expect(spreading.slide).toBe(1);
    expect(spreading.scale).toBeGreaterThan(1);
    expect(spreading.alpha).toBeLessThan(GHOST_ALPHA);
    expect(ghostPhase(update, 1700 + 75).swell).toBeCloseTo(1.25);
    expect(ghostPhase(update, 1700 + 150).done).toBe(true);
  });

  it("gives each kind a different motion", () => {
    const mid = (ghost: ReplayGhost) => ghostPhase(ghost, 1100 + 500);
    expect(mid(merge()).slide).toBeGreaterThan(0.5);
    expect(mid(replace).slide).toBeLessThan(0);
    expect(mid(update).slide).toBe(1);
    expect(mid(update).scale).toBeGreaterThan(2);
  });

  it("swells only what it slides into, and stops painting once all are in", () => {
    expect(swellOf([merge()], "m1", 1775)).toBeCloseTo(1.25);
    expect(swellOf([merge()], "other", 1775)).toBe(1);
    expect(ghostsMoving([merge(), merge(110)], 1850)).toBe(true);
    expect(ghostsMoving([merge(), merge(110)], 1960)).toBe(false);
  });

  it("keeps still ghosts at rest with reduced motion, until Replay is done", () => {
    const still = { ...merge(), still: true };
    expect(ghostPhase(still, 10_000)).toMatchObject({ alpha: GHOST_ALPHA, slide: 0, swell: 1, done: false });
    expect(ghostPhase({ ...update, still: true }, 10_000)).toMatchObject({ slide: 1, done: false });
    expect(ghostsMoving([still], 10_000)).toBe(false);
  });

  it("puts a ghost's words on one line, cut to fit", () => {
    const measure = (text: string) => text.length * 10;
    expect(saidLine("short", measure)).toBe("short");
    expect(saidLine("\n first line \nsecond", measure)).toBe("first line");
    const cut = saidLine("x".repeat(40), measure, 100);
    expect(cut.endsWith("…")).toBe(true);
    expect(measure(cut)).toBeLessThanOrEqual(100);
  });
});
