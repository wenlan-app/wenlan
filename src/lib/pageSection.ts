// SPDX-License-Identifier: AGPL-3.0-only
//
// `[[Page#Heading]]` opens Page at that heading. The link that is clicked
// lives on one page and the heading on the next one opened, so the heading
// waits here for that page to show it.

let pending: { pageId: string; heading: string } | null = null;

/** Remember to show `heading` once page `pageId` opens. */
export function requestPageSection(pageId: string, heading: string | null | undefined): void {
  const text = heading?.trim();
  pending = text ? { pageId, heading: text } : null;
}

/** The heading waiting for page `pageId`, if any, without taking it. */
export function pendingPageSection(pageId: string): string | null {
  return pending?.pageId === pageId ? pending.heading : null;
}

/** Forget the heading waiting for page `pageId`. */
export function clearPageSection(pageId: string): void {
  if (pending?.pageId === pageId) pending = null;
}

/** Plain words of a heading, for comparing a link's `#heading` with the page. */
export function headingKey(text: string): string {
  return text
    .replace(/^\s*#{1,6}\s+/, "")
    .replace(/\s+#+\s*$/, "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

/** The `#heading` of a wikilink's inner text, or null. */
export function wikilinkHeading(inner: string): string | null {
  const pipe = inner.indexOf("|");
  const target = pipe >= 0 ? inner.slice(0, pipe) : inner;
  const hash = target.indexOf("#");
  const heading = hash >= 0 ? target.slice(hash + 1).trim() : "";
  return heading || null;
}
