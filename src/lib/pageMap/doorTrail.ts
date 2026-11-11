// SPDX-License-Identifier: AGPL-3.0-only
//
// Where the mind map has been walked from, for this session only.
//
// A grey link inside an opened door re-roots the map on that page. The page it
// left is remembered here so the map bar can offer "← <that page>". Nothing is
// stored: a restart forgets the trail, and a hop only counts while the person
// is still on the page it led to.

interface Hop {
  from: { id: string; title: string };
  to: string;
}

let hops: Hop[] = [];

export function pushHop(from: { id: string; title: string }, to: string): void {
  hops.push({ from, to });
  if (hops.length > 50) hops = hops.slice(-50);
}

/** The page the person walked here from, when they did. */
export function hopBack(currentPageId: string): { id: string; title: string } | null {
  for (let i = hops.length - 1; i >= 0; i -= 1) {
    if (hops[i].to === currentPageId) return hops[i].from;
  }
  return null;
}

/** Take the last hop that led to this page off the trail. */
export function popHop(currentPageId: string): void {
  for (let i = hops.length - 1; i >= 0; i -= 1) {
    if (hops[i].to === currentPageId) {
      hops.splice(i, 1);
      return;
    }
  }
}

export function clearTrail(): void {
  hops = [];
  pendingJump = null;
}

/** A heading or item of another page, to scroll to once that page opens. */
export interface SectionJump {
  pageId: string;
  text: string;
  heading: string | null;
}

let pendingJump: SectionJump | null = null;

export function setPendingJump(jump: SectionJump): void {
  pendingJump = jump;
}

/** The jump waiting for this page, taken so it happens once. */
export function takePendingJump(pageId: string): SectionJump | null {
  if (pendingJump?.pageId !== pageId) return null;
  const jump = pendingJump;
  pendingJump = null;
  return jump;
}
