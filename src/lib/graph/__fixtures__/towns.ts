// SPDX-License-Identifier: AGPL-3.0-only
// Round 3 test map: six towns in a chain, one hub with about 1000 links and
// one with about 250, and 300 small islands. Every name is invented.
import Graph from "graphology";

/** Small seeded generator so the fixture is the same on every run. */
function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

const TOWNS = ["Ashford Vale", "Brindle Cross", "Corrow Field", "Dunmere", "Elkwater", "Fennick Row"];

export type TownsOptions = { scale?: number; seed?: number };

/** `scale` shrinks every count (1 is the full ~5k-node map). */
export function townsGraph({ scale = 1, seed = 7 }: TownsOptions = {}): Graph {
  const rand = seeded(seed);
  const graph = new Graph({ multi: true });
  const n = (count: number) => Math.max(1, Math.round(count * scale));
  let made = 0;
  const add = (id: string, entityType = "page") => {
    const angle = made++ * 2.39996;
    graph.addNode(id, { x: Math.cos(angle) * 100, y: Math.sin(angle) * 100, size: 4, entityType, label: id });
    return id;
  };
  const link = (a: string, b: string) => graph.addEdge(a, b, { edgeType: "wikilink" });

  const centres: string[] = [];
  TOWNS.forEach((town, t) => {
    const centre = add(`${town} Hall`);
    centres.push(centre);
    // The first town's hall is the 1000-link hub, the fourth's the 250-link one.
    const members = t === 0 ? n(1000) : t === 3 ? n(250) : n(120);
    const streets: string[] = [];
    for (let i = 0; i < members; i += 1) {
      const id = add(`${town} lane ${i}`);
      link(centre, id);
      streets.push(id);
    }
    // Some cross-links inside a town, so it is not a pure star.
    for (let i = 0; i < Math.floor(members * 1.5); i += 1) {
      const a = streets[Math.floor(rand() * streets.length)];
      const b = streets[Math.floor(rand() * streets.length)];
      if (a !== b) link(a, b);
    }
    // A few local hubs per town, each with its own small street.
    for (let h = 0; h < 4; h += 1) {
      const hub = add(`${town} market ${h}`);
      link(centre, hub);
      for (let i = 0; i < n(40); i += 1) {
        const id = add(`${town} market ${h} stall ${i}`);
        link(hub, id);
        if (rand() < 0.3) link(id, streets[Math.floor(rand() * streets.length)]);
      }
    }
  });
  // The chain: each hall links to the next, with a few roads between towns.
  for (let t = 0; t + 1 < centres.length; t += 1) {
    link(centres[t], centres[t + 1]);
    for (let r = 0; r < n(30); r += 1) link(`${TOWNS[t]} lane ${r % n(120)}`, `${TOWNS[t + 1]} lane ${r % n(120)}`);
  }
  // 300 islands of 1-6 nodes, unlinked to the towns.
  for (let i = 0; i < n(300); i += 1) {
    const size = 1 + Math.floor(rand() * 6);
    const first = add(`Outlying farm ${i}`);
    for (let k = 1; k < size; k += 1) link(first, add(`Outlying farm ${i} field ${k}`));
  }
  return graph;
}
